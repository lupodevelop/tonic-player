// What the modules share: the open video, the Cast session, the VR layout
// and what the bar shows. Modules cannot assign an imported binding, so the
// mutable state lives on these two objects.
import type { Vr } from "./vr";

export type { Vr };
export type Episode = { file_id: number; name: string; title: string; size: number };
export type Opened = {
  name: string;
  path: string;
  /** Preview frame path; `{t}` is the time in seconds. */
  thumbs: string;
  source: string;
  host: string;
  port: number;
  torrent: number | null;
  file_id: number | null;
  vr: Vr | null;
  chapters: { start: number; end: number; title: string }[];
  /** Videos of the torrent; when there are several and `path` is empty, ask. */
  episodes: Episode[];
  /** Folder the episodes come from; their `name` is then a file path. */
  folder: string | null;
};
export type Device = { name: string; ip: string; port: number };

export const app = {
  current: null as Opened | null,
  /** Name of the device being cast to, while a Cast session runs. */
  casting: null as string | null,
  castDevice: null as Device | null,
  /** Id of the running Cast session; events of any other session are stale. */
  castSession: null as number | null,
  /** Whether the window is the mini player. */
  mini: false,
  vrLayout: null as Vr | null,
  vrFlat: false,
  /** Whether the seek bar is being dragged. */
  seeking: false,
};

/** What the bar shows, from the video here or from the Cast device. */
export const ui = { time: 0, duration: 0, paused: true, volume: 1, muted: false };
