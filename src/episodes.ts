// Folders and torrents with several videos: pick one; the bar's list button
// switches later. Duration and the title tag come from each file's header,
// one file at a time, while the list is open.
import { invoke } from "@tauri-apps/api/core";
import { $, chooser, row } from "./dom";
import { clock, size } from "./format";
import { open } from "./playback";
import { app, type Episode, type Opened } from "./state";

type EpisodeInfo = { duration: number | null; title: string | null };

let choosing: Opened | null = null;
const infos = new Map<string, EpisodeInfo>();
const infoKey = (source: Opened, episode: Episode) => (source.folder ? episode.name : `${source.torrent}:${episode.file_id}`);

export function showChooser(source: Opened) {
  choosing = source;
  const current = app.current;
  const sameSource = source.folder ? current?.folder === source.folder : current?.torrent === source.torrent;
  const list = $("episodes-list");
  list.replaceChildren(
    ...source.episodes.map((episode) => {
      const button = document.createElement("button");
      button.type = "button";
      button.dataset.file = String(episode.file_id);
      button.setAttribute("aria-current", String(sameSource && current?.file_id === episode.file_id));
      const name = Object.assign(document.createElement("span"), { className: "name" });
      const title = Object.assign(document.createElement("span"), { className: "title", textContent: episode.title });
      const path = Object.assign(document.createElement("span"), { className: "path", textContent: episode.name });
      name.append(title, path);
      const meta = Object.assign(document.createElement("span"), { className: "size", textContent: size(episode.size) });
      button.append(name, meta);
      button.addEventListener("click", () => pickEpisode(episode, source));
      showInfo(button, source, episode);
      return row(button);
    }),
  );
  chooser.hidden = false;
  list.querySelector("button")?.focus();
  probeEpisodes(source);
}

function showInfo(button: HTMLElement, source: Opened, episode: Episode) {
  const info = infos.get(infoKey(source, episode));
  if (!info) return;
  if (info.title) button.querySelector(".title")!.textContent = info.title;
  if (info.duration) button.querySelector(".size")!.textContent = `${clock(info.duration)} · ${size(episode.size)}`;
}

async function probeEpisodes(source: Opened) {
  for (const episode of source.episodes) {
    if (choosing !== source) return;
    const key = infoKey(source, episode);
    if (!infos.has(key)) {
      const info = await (source.folder
        ? invoke<EpisodeInfo>("file_info", { path: episode.name })
        : invoke<EpisodeInfo>("episode_info", { torrent: source.torrent, fileId: episode.file_id })
      ).catch(() => ({ duration: null, title: null }));
      infos.set(key, info);
    }
    const button = $("episodes-list").querySelector<HTMLElement>(`button[data-file="${episode.file_id}"]`);
    if (button) showInfo(button, source, episode);
  }
}

export function hideChooser() {
  chooser.hidden = true;
  choosing = null;
}

/** Plays one video of `source`, under its title tag when the header has one. */
export function pickEpisode(episode: Episode, source: Opened) {
  if (source.torrent === null && !source.folder) return;
  hideChooser();
  return open("Opening…", async () => {
    // A folder's video opens like a dropped file and keeps the folder's list.
    const opened = source.folder
      ? {
          ...(await invoke<Opened>("open", { input: episode.name })),
          episodes: source.episodes,
          folder: source.folder,
          file_id: episode.file_id,
        }
      : await invoke<Opened>("open_episode", {
          torrent: source.torrent,
          fileId: episode.file_id,
          episodes: source.episodes,
        });
    return { ...opened, name: infos.get(infoKey(source, episode))?.title || opened.name };
  });
}

$("episodes").addEventListener("click", () => app.current && showChooser(app.current));
chooser.addEventListener("click", (e) => {
  if (e.target === chooser && app.current) hideChooser();
});
