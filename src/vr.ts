import { EASU, frameClock, linkProgram } from "./upscale";

/** Layout of a VR video, as the backend reports it. */
export type Vr = { fov: 180 | 360; stereo: "mono" | "sbs" | "tb" };

// One full-screen triangle pair. For each pixel the shader either casts a ray
// into the sphere (look around) or maps the pixel onto one eye's frame (2D).
// Texture rows run top to bottom, so v = 0 is the top of the frame.
const FRAGMENT = `
precision highp float;
varying vec2 uv;
uniform sampler2D frame;
uniform float yaw, pitch, tanHalf, aspect, eyeAspect;
uniform int sphere, half360, stereo;
const float PI = 3.14159265;

vec4 sampleEye(vec2 t) {
  if (t.x < 0.0 || t.x > 1.0 || t.y < 0.0 || t.y > 1.0) return vec4(0.0, 0.0, 0.0, 1.0);
  if (stereo == 1) t.x *= 0.5;
  if (stereo == 2) t.y *= 0.5;
  return texture2D(frame, t);
}

void main() {
  vec2 p = uv * 2.0 - 1.0;
  if (sphere == 0) {
    vec2 s = p;
    if (aspect > eyeAspect) s.x *= aspect / eyeAspect; else s.y *= eyeAspect / aspect;
    gl_FragColor = sampleEye(vec2(s.x * 0.5 + 0.5, 0.5 - s.y * 0.5));
    return;
  }
  vec3 d = normalize(vec3(p.x * tanHalf * aspect, p.y * tanHalf, -1.0));
  float cp = cos(pitch), sp = sin(pitch);
  d = vec3(d.x, d.y * cp - d.z * sp, d.y * sp + d.z * cp);
  float cy = cos(yaw), sy = sin(yaw);
  d = vec3(d.x * cy - d.z * sy, d.y, d.x * sy + d.z * cy);
  float lon = atan(d.x, -d.z);
  float lat = asin(clamp(d.y, -1.0, 1.0));
  if (half360 == 1 && abs(lon) > PI * 0.5) { gl_FragColor = vec4(0.0, 0.0, 0.0, 1.0); return; }
  float u = half360 == 1 ? lon / PI + 0.5 : lon / (2.0 * PI) + 0.5;
  gl_FragColor = sampleEye(vec2(u, 0.5 - lat / PI));
}`;

/**
 * Draws a VR video from `video` onto `canvas`: as a view into the sphere that
 * drag and wheel move, or flat (one eye, whole frame) in 2D mode.
 */
export class VrView {
  layout: Vr | null = null;
  flat = false;
  /** Scale the frame up 2x with FSR's EASU before projecting it, when the result fits in 4096 px. */
  upscale = true;
  /** A click without dragging. */
  onTap?: () => void;
  private yaw = 0;
  private pitch = 0;
  private fov = 90;
  private gl: WebGLRenderingContext;
  private program: WebGLProgram;
  private easu!: WebGLProgram;
  private raw!: WebGLTexture;
  private up!: WebGLTexture;
  private fbo!: WebGLFramebuffer;
  private upSize = "";
  private lastFrame = -1;
  private frameTime: () => number;
  private dirty = true;
  private running = false;
  private textureSize = "";
  private uniforms = new Map<string, WebGLUniformLocation | null>();

  constructor(private canvas: HTMLCanvasElement, private video: HTMLVideoElement) {
    this.frameTime = frameClock(video);
    const gl = canvas.getContext("webgl", { antialias: false });
    if (!gl) throw new Error("WebGL is not available");
    this.gl = gl;
    this.program = this.setup();
    this.listen();
    // A new source must be uploaded even if its first frame time matches the old one.
    for (const event of ["emptied", "loadeddata", "seeked"]) {
      video.addEventListener(event, () => {
        this.lastFrame = -1;
        this.dirty = true;
      });
    }
    // The GPU can drop the context (sleep, large frames): rebuild it when it returns.
    canvas.addEventListener("webglcontextlost", (e) => e.preventDefault());
    canvas.addEventListener("webglcontextrestored", () => {
      this.program = this.setup();
      this.uniforms.clear();
      this.textureSize = "";
      this.upSize = "";
      this.lastFrame = -1;
      this.dirty = true;
    });
  }

  /** Programs, full-screen quad, the video texture and the upscaled one. */
  private setup(): WebGLProgram {
    const gl = this.gl;
    const program = linkProgram(gl, FRAGMENT);
    this.easu = linkProgram(gl, EASU);
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    const texture = () => {
      const t = gl.createTexture()!;
      gl.bindTexture(gl.TEXTURE_2D, t);
      for (const [k, v] of [
        [gl.TEXTURE_MIN_FILTER, gl.LINEAR],
        [gl.TEXTURE_MAG_FILTER, gl.LINEAR],
        [gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE],
        [gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE],
      ]) {
        gl.texParameteri(gl.TEXTURE_2D, k, v);
      }
      return t;
    };
    this.up = texture();
    this.raw = texture();
    this.fbo = gl.createFramebuffer()!;
    return program;
  }

  /** The 2x frame size when upscaling applies to this video, else null. */
  private upscaledSize() {
    const w = this.video.videoWidth * 2;
    const h = this.video.videoHeight * 2;
    return this.upscale && w <= 4096 && h <= 4096 ? { w, h } : null;
  }

  /** Shows `layout` (null hides the canvas) and resets the view. */
  show(layout: Vr | null) {
    this.layout = layout;
    this.lastFrame = -1;
    this.yaw = 0;
    this.pitch = 0;
    this.fov = 90;
    this.canvas.hidden = layout === null;
    this.dirty = true;
    if (layout && !this.running) {
      this.running = true;
      requestAnimationFrame(() => this.frame());
    }
  }

  setUpscale(on: boolean) {
    this.upscale = on;
    this.lastFrame = -1;
    this.dirty = true;
  }

  setFlat(flat: boolean) {
    this.flat = flat;
    this.dirty = true;
  }

  private listen() {
    let from: { x: number; y: number } | null = null;
    let moved = 0;
    this.canvas.addEventListener("pointerdown", (e) => {
      from = { x: e.clientX, y: e.clientY };
      moved = 0;
      this.canvas.setPointerCapture(e.pointerId);
    });
    this.canvas.addEventListener("pointermove", (e) => {
      if (!from) return;
      moved += Math.abs(e.clientX - from.x) + Math.abs(e.clientY - from.y);
      if (!this.flat) {
        // Dragging across the window height turns the view by the field of view.
        const perPixel = this.fov / this.canvas.clientHeight;
        this.yaw -= (e.clientX - from.x) * perPixel;
        this.pitch = Math.max(-89, Math.min(89, this.pitch + (e.clientY - from.y) * perPixel));
        this.dirty = true;
      }
      from = { x: e.clientX, y: e.clientY };
    });
    this.canvas.addEventListener("pointerup", () => {
      from = null;
      if (moved < 4) this.onTap?.();
    });
    this.canvas.addEventListener(
      "wheel",
      (e) => {
        if (this.flat) return;
        e.preventDefault();
        this.fov = Math.max(30, Math.min(120, this.fov + e.deltaY * 0.05));
        this.dirty = true;
      },
      { passive: false },
    );
  }

  private frame() {
    if (!this.layout) {
      this.running = false;
      return;
    }
    requestAnimationFrame(() => this.frame());
    const { gl, video, canvas } = this;
    const scale = Math.min(window.devicePixelRatio || 1, 2);
    const w = Math.round(canvas.clientWidth * scale);
    const h = Math.round(canvas.clientHeight * scale);
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
      this.dirty = true;
    }
    const up = this.upscaledSize();
    const time = this.frameTime();
    if (video.readyState >= 2 && time !== this.lastFrame) {
      // Allocate only when the frame size changes; otherwise overwrite in place.
      gl.bindTexture(gl.TEXTURE_2D, this.raw);
      const size = `${video.videoWidth}x${video.videoHeight}`;
      if (size !== this.textureSize) {
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGB, gl.RGB, gl.UNSIGNED_BYTE, video);
        this.textureSize = size;
      } else {
        gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, gl.RGB, gl.UNSIGNED_BYTE, video);
      }
      if (up) {
        gl.bindTexture(gl.TEXTURE_2D, this.up);
        if (this.upSize !== `${up.w}x${up.h}`) {
          this.upSize = `${up.w}x${up.h}`;
          gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, up.w, up.h, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
          gl.bindFramebuffer(gl.FRAMEBUFFER, this.fbo);
          gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, this.up, 0);
        }
        gl.bindFramebuffer(gl.FRAMEBUFFER, this.fbo);
        gl.viewport(0, 0, up.w, up.h);
        gl.useProgram(this.easu);
        gl.bindTexture(gl.TEXTURE_2D, this.raw);
        gl.uniform1i(gl.getUniformLocation(this.easu, "src"), 0);
        gl.uniform2f(gl.getUniformLocation(this.easu, "inSize"), video.videoWidth, video.videoHeight);
        gl.uniform1f(gl.getUniformLocation(this.easu, "flipY"), 0);
        gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
        gl.bindFramebuffer(gl.FRAMEBUFFER, null);
      }
      this.lastFrame = time;
      this.dirty = true;
    }
    if (!this.dirty || !video.videoWidth) return;
    this.dirty = false;
    gl.useProgram(this.program);
    gl.bindTexture(gl.TEXTURE_2D, up ? this.up : this.raw);
    const { stereo, fov } = this.layout;
    const eyeW = video.videoWidth / (stereo === "sbs" ? 2 : 1);
    const eyeH = video.videoHeight / (stereo === "tb" ? 2 : 1);
    const u = (name: string) => {
      let at = this.uniforms.get(name);
      if (at === undefined) {
        at = gl.getUniformLocation(this.program, name);
        this.uniforms.set(name, at);
      }
      return at;
    };
    const rad = Math.PI / 180;
    gl.viewport(0, 0, w, h);
    gl.uniform1f(u("yaw"), this.yaw * rad);
    gl.uniform1f(u("pitch"), this.pitch * rad);
    gl.uniform1f(u("tanHalf"), Math.tan((this.fov * rad) / 2));
    gl.uniform1f(u("aspect"), w / h);
    gl.uniform1f(u("eyeAspect"), eyeW / eyeH);
    gl.uniform1i(u("sphere"), this.flat ? 0 : 1);
    gl.uniform1i(u("half360"), fov === 180 ? 1 : 0);
    gl.uniform1i(u("stereo"), stereo === "sbs" ? 1 : stereo === "tb" ? 2 : 0);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
  }
}
