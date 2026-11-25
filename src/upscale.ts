// AMD FidelityFX Super Resolution 1.0 (MIT licence, Copyright (c) 2021
// Advanced Micro Devices, Inc.): EASU scales the frame up along its edges,
// RCAS then sharpens it. Both passes are ports of the reference shaders.

export const VERTEX = `
attribute vec2 corner;
varying vec2 uv;
void main() {
  uv = corner * 0.5 + 0.5;
  gl_Position = vec4(corner, 0.0, 1.0);
}`;

// The video texture has its top row at v = 0. With flipY = 1 EASU writes in
// GL's own orientation, which RCAS keeps onto the canvas; with flipY = 0 the
// output keeps the video's orientation, for the VR view to sample.
export const EASU = `
precision highp float;
varying vec2 uv;
uniform sampler2D src;
uniform vec2 inSize;
uniform float flipY;

float luma(vec3 c) { return c.b * 0.5 + (c.r * 0.5 + c.g); }

void dir4(inout vec2 dir, inout float len, float w, float a, float b, float c, float d, float e) {
  float dc = d - c, cb = c - b;
  float lx = max(abs(dc), abs(cb));
  lx = lx > 0.0 ? 1.0 / lx : 0.0;
  float dx = d - b;
  dir.x += dx * w;
  lx = clamp(abs(dx) * lx, 0.0, 1.0);
  len += lx * lx * w;
  float ec = e - c, ca = c - a;
  float ly = max(abs(ec), abs(ca));
  ly = ly > 0.0 ? 1.0 / ly : 0.0;
  float dy = e - a;
  dir.y += dy * w;
  ly = clamp(abs(dy) * ly, 0.0, 1.0);
  len += ly * ly * w;
}

void tap(inout vec3 acc, inout float aw, vec2 off, vec2 dir, vec2 len2, float lob, float clp, vec3 c) {
  vec2 v = vec2(dot(off, dir), dot(off, vec2(-dir.y, dir.x))) * len2;
  float d2 = min(dot(v, v), clp);
  float wb = 0.4 * d2 - 1.0;
  float wa = lob * d2 - 1.0;
  wb *= wb;
  wa *= wa;
  float w = (1.5625 * wb - 0.5625) * wa;
  acc += c * w;
  aw += w;
}

void main() {
  vec2 pp = vec2(uv.x, mix(uv.y, 1.0 - uv.y, flipY)) * inSize - 0.5;
  vec2 fp = floor(pp);
  pp -= fp;
  vec2 px = 1.0 / inSize;
  vec2 o = (fp + 0.5) * px;
  //    b c
  //  e f g h
  //  i j k l
  //    n o
  vec3 b = texture2D(src, o + vec2(0.0, -1.0) * px).rgb;
  vec3 c = texture2D(src, o + vec2(1.0, -1.0) * px).rgb;
  vec3 e = texture2D(src, o + vec2(-1.0, 0.0) * px).rgb;
  vec3 f = texture2D(src, o).rgb;
  vec3 g = texture2D(src, o + vec2(1.0, 0.0) * px).rgb;
  vec3 h = texture2D(src, o + vec2(2.0, 0.0) * px).rgb;
  vec3 i = texture2D(src, o + vec2(-1.0, 1.0) * px).rgb;
  vec3 j = texture2D(src, o + vec2(0.0, 1.0) * px).rgb;
  vec3 k = texture2D(src, o + vec2(1.0, 1.0) * px).rgb;
  vec3 l = texture2D(src, o + vec2(2.0, 1.0) * px).rgb;
  vec3 n = texture2D(src, o + vec2(0.0, 2.0) * px).rgb;
  vec3 q = texture2D(src, o + vec2(1.0, 2.0) * px).rgb;
  float bL = luma(b), cL = luma(c), eL = luma(e), fL = luma(f), gL = luma(g), hL = luma(h);
  float iL = luma(i), jL = luma(j), kL = luma(k), lL = luma(l), nL = luma(n), qL = luma(q);

  vec2 dir = vec2(0.0);
  float len = 0.0;
  dir4(dir, len, (1.0 - pp.x) * (1.0 - pp.y), bL, eL, fL, gL, jL);
  dir4(dir, len, pp.x * (1.0 - pp.y), cL, fL, gL, hL, kL);
  dir4(dir, len, (1.0 - pp.x) * pp.y, fL, iL, jL, kL, nL);
  dir4(dir, len, pp.x * pp.y, gL, jL, kL, lL, qL);

  float dr = dot(dir, dir);
  bool none = dr < 1.0 / 32768.0;
  dir = none ? vec2(1.0, 0.0) : dir * inversesqrt(dr);
  len = len * 0.5;
  len *= len;
  float stretch = dot(dir, dir) / max(abs(dir.x), abs(dir.y));
  vec2 len2 = vec2(1.0 + (stretch - 1.0) * len, 1.0 - 0.5 * len);
  float lob = 0.5 + ((1.0 / 4.0 - 0.04) - 0.5) * len;
  float clp = 1.0 / lob;

  vec3 acc = vec3(0.0);
  float aw = 0.0;
  tap(acc, aw, vec2(0.0, -1.0) - pp, dir, len2, lob, clp, b);
  tap(acc, aw, vec2(1.0, -1.0) - pp, dir, len2, lob, clp, c);
  tap(acc, aw, vec2(-1.0, 1.0) - pp, dir, len2, lob, clp, i);
  tap(acc, aw, vec2(0.0, 1.0) - pp, dir, len2, lob, clp, j);
  tap(acc, aw, vec2(0.0, 0.0) - pp, dir, len2, lob, clp, f);
  tap(acc, aw, vec2(-1.0, 0.0) - pp, dir, len2, lob, clp, e);
  tap(acc, aw, vec2(1.0, 1.0) - pp, dir, len2, lob, clp, k);
  tap(acc, aw, vec2(2.0, 1.0) - pp, dir, len2, lob, clp, l);
  tap(acc, aw, vec2(2.0, 0.0) - pp, dir, len2, lob, clp, h);
  tap(acc, aw, vec2(1.0, 0.0) - pp, dir, len2, lob, clp, g);
  tap(acc, aw, vec2(1.0, 2.0) - pp, dir, len2, lob, clp, q);
  tap(acc, aw, vec2(0.0, 2.0) - pp, dir, len2, lob, clp, n);

  // The result never leaves the range of the four nearest pixels (no ringing).
  vec3 lo = min(min(f, g), min(j, k));
  vec3 hi = max(max(f, g), max(j, k));
  gl_FragColor = vec4(clamp(acc / aw, lo, hi), 1.0);
}`;

const RCAS = `
precision highp float;
varying vec2 uv;
uniform sampler2D src;
uniform vec2 size;
uniform float sharpness;

void main() {
  vec2 px = 1.0 / size;
  vec2 t = uv;
  vec3 b = texture2D(src, t + vec2(0.0, -px.y)).rgb;
  vec3 d = texture2D(src, t + vec2(-px.x, 0.0)).rgb;
  vec3 e = texture2D(src, t).rgb;
  vec3 f = texture2D(src, t + vec2(px.x, 0.0)).rgb;
  vec3 h = texture2D(src, t + vec2(0.0, px.y)).rgb;
  vec3 mn = min(min(b, d), min(f, h));
  vec3 mx = max(max(b, d), max(f, h));
  vec3 hitMin = min(mn, e) / (4.0 * max(mx, e) + 1e-5);
  vec3 hitMax = (1.0 - max(mx, e)) / (4.0 * min(mn, e) - 4.0 - 1e-5);
  vec3 lobes = max(-hitMin, hitMax);
  float lobe = max(-0.1875, min(max(lobes.r, max(lobes.g, lobes.b)), 0.0)) * exp2(-sharpness);
  gl_FragColor = vec4((lobe * (b + d + f + h) + e) / (4.0 * lobe + 1.0), 1.0);
}`;

/**
 * Media time of the latest frame `video` presented, to upload each frame
 * exactly once. requestVideoFrameCallback reports every frame; currentTime,
 * the fallback, can move between frames or lag behind them.
 */
export function frameClock(video: HTMLVideoElement): () => number {
  if (typeof video.requestVideoFrameCallback !== "function") return () => video.currentTime;
  let time = -1;
  const next = () =>
    video.requestVideoFrameCallback((_, frame) => {
      time = frame.mediaTime;
      next();
    });
  next();
  return () => time;
}

/** Links `fragment` with the full-screen quad shader; `corner` is attribute 0. */
export function linkProgram(gl: WebGLRenderingContext, fragment: string): WebGLProgram {
  const program = gl.createProgram()!;
  for (const [type, text] of [
    [gl.VERTEX_SHADER, VERTEX],
    [gl.FRAGMENT_SHADER, fragment],
  ] as const) {
    const shader = gl.createShader(type)!;
    gl.shaderSource(shader, text);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(shader) ?? "shader");
    gl.attachShader(program, shader);
  }
  gl.bindAttribLocation(program, 0, "corner");
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(program) ?? "link");
  return program;
}

/**
 * Draws `video` onto `canvas` scaled up with FSR when the picture on screen
 * is larger than the video. The canvas sits over the video's letterboxed
 * area and passes pointer events through; it hides itself whenever the
 * video is not scaled up, or `enabled` is off.
 */
export class Upscaler {
  enabled = true;
  /** Set by the owner when another view (VR, casting) owns the picture. */
  blocked = false;
  private gl: WebGLRenderingContext;
  private easu!: WebGLProgram;
  private rcas!: WebGLProgram;
  private source!: WebGLTexture;
  private target!: WebGLTexture;
  private fbo!: WebGLFramebuffer;
  private sourceSize = "";
  private targetSize = "";
  private lastFrame = -1;
  private uniforms = new Map<string, WebGLUniformLocation | null>();
  private frameTime: () => number;

  constructor(private canvas: HTMLCanvasElement, private video: HTMLVideoElement) {
    this.frameTime = frameClock(video);
    const gl = canvas.getContext("webgl", { antialias: false, alpha: false });
    if (!gl) throw new Error("WebGL is not available");
    this.gl = gl;
    this.setup();
    for (const event of ["emptied", "loadeddata", "seeked"]) video.addEventListener(event, () => (this.lastFrame = -1));
    canvas.addEventListener("webglcontextlost", (e) => e.preventDefault());
    canvas.addEventListener("webglcontextrestored", () => this.setup());
    requestAnimationFrame(() => this.frame());
  }

  private setup() {
    const gl = this.gl;
    this.easu = this.link(EASU);
    this.rcas = this.link(RCAS);
    this.uniforms.clear();
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
    for (const program of [this.easu, this.rcas]) {
      const corner = gl.getAttribLocation(program, "corner");
      gl.enableVertexAttribArray(corner);
      gl.vertexAttribPointer(corner, 2, gl.FLOAT, false, 0, 0);
    }
    this.source = this.texture();
    this.target = this.texture();
    this.fbo = gl.createFramebuffer()!;
    this.sourceSize = "";
    this.targetSize = "";
    this.lastFrame = -1;
  }

  private texture(): WebGLTexture {
    const gl = this.gl;
    const t = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, t);
    for (const [k, v] of [
      [gl.TEXTURE_MIN_FILTER, gl.NEAREST],
      [gl.TEXTURE_MAG_FILTER, gl.NEAREST],
      [gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE],
      [gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE],
    ]) {
      gl.texParameteri(gl.TEXTURE_2D, k, v);
    }
    return t;
  }

  private link(fragment: string): WebGLProgram {
    return linkProgram(this.gl, fragment);
  }

  private uniform(program: WebGLProgram, name: string) {
    const key = `${program === this.easu ? "e" : "r"}:${name}`;
    if (!this.uniforms.has(key)) this.uniforms.set(key, this.gl.getUniformLocation(program, name));
    return this.uniforms.get(key) ?? null;
  }

  /** Whether the video is on screen at a larger size than it has. */
  private placement() {
    const { video } = this;
    const vw = video.videoWidth;
    const vh = video.videoHeight;
    if (!this.enabled || this.blocked || video.hidden || !vw || !vh || video.readyState < 2) return null;
    // HD and larger already look right scaled by the compositor; FSR there only costs frames.
    if (Math.min(vw, vh) >= 1000) return null;
    const box = video.getBoundingClientRect();
    const scale = Math.min(box.width / vw, box.height / vh);
    const dpr = window.devicePixelRatio || 1;
    // Off when the screen shows the video at (nearly) its own size or smaller.
    if (scale * dpr < 1.25) return null;
    const width = vw * scale;
    const height = vh * scale;
    return {
      left: box.left + (box.width - width) / 2,
      top: box.top + (box.height - height) / 2,
      width,
      height,
      pw: Math.min(Math.round(width * dpr), 3840),
      ph: Math.min(Math.round(height * dpr), 2160),
    };
  }

  private frame() {
    requestAnimationFrame(() => this.frame());
    const place = this.placement();
    if (this.canvas.hidden !== (place === null)) this.canvas.hidden = place === null;
    if (!place) {
      this.lastFrame = -1;
      return;
    }
    const { gl, canvas, video } = this;
    Object.assign(canvas.style, {
      left: `${place.left}px`,
      top: `${place.top}px`,
      width: `${place.width}px`,
      height: `${place.height}px`,
    });
    const resized = canvas.width !== place.pw || canvas.height !== place.ph;
    if (resized) {
      canvas.width = place.pw;
      canvas.height = place.ph;
    }
    const time = this.frameTime();
    if (!resized && time === this.lastFrame) return;
    this.lastFrame = time;

    const vw = video.videoWidth;
    const vh = video.videoHeight;
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.source);
    if (this.sourceSize !== `${vw}x${vh}`) {
      this.sourceSize = `${vw}x${vh}`;
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, video);
    } else {
      gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, gl.RGBA, gl.UNSIGNED_BYTE, video);
    }

    gl.bindTexture(gl.TEXTURE_2D, this.target);
    if (this.targetSize !== `${place.pw}x${place.ph}`) {
      this.targetSize = `${place.pw}x${place.ph}`;
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, place.pw, place.ph, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
      gl.bindFramebuffer(gl.FRAMEBUFFER, this.fbo);
      gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, this.target, 0);
    }

    gl.viewport(0, 0, place.pw, place.ph);
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.fbo);
    gl.useProgram(this.easu);
    gl.bindTexture(gl.TEXTURE_2D, this.source);
    gl.uniform1i(this.uniform(this.easu, "src"), 0);
    gl.uniform2f(this.uniform(this.easu, "inSize"), vw, vh);
    gl.uniform1f(this.uniform(this.easu, "flipY"), 1);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);

    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    gl.useProgram(this.rcas);
    gl.bindTexture(gl.TEXTURE_2D, this.target);
    gl.uniform1i(this.uniform(this.rcas, "src"), 0);
    gl.uniform2f(this.uniform(this.rcas, "size"), place.pw, place.ph);
    gl.uniform1f(this.uniform(this.rcas, "sharpness"), 0.2);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
  }
}
