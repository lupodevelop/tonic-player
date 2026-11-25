// What Tonic Player is and does: the page, its structured data and llms.txt read it from here.
import type Icon from './components/Icon.astro';

export const repo = 'https://github.com/lupodevelop/tonic-player';
export const description =
  'Video player for local files and torrents, streamed while they download, with Chromecast and AirPlay. Free software for macOS, Windows and Linux.';

type Feature = { icon: Parameters<typeof Icon>[0]['name']; title: string; text: string };

export const groups: { name: string; items: Feature[] }[] = [
  {
    name: 'Play',
    items: [
      { icon: 'drop', title: 'Drop and play', text: 'Drop a video, a folder or a .torrent on the window. Magnet links open from the browser.' },
      { icon: 'stream', title: 'Torrents while they download', text: 'Playback starts before the download ends; folders and multi-video torrents show a list.' },
      { icon: 'film', title: 'Any format', text: 'MKV, AVI and others are converted on the fly, on the GPU where there is one, with free seeking.' },
    ],
  },
  {
    name: 'Watch your way',
    items: [
      { icon: 'globe', title: '360° and 180° video', text: 'Drag to look around, or watch one eye flat.' },
      { icon: 'cast', title: 'Chromecast', text: 'The control bar drives the device; when the cast ends, playback goes on in the window.' },
      { icon: 'airplay', title: 'AirPlay', text: 'Send the video to AirPlay devices on macOS.' },
    ],
  },
  {
    name: 'Picture',
    items: [
      { icon: 'upscale', title: 'Upscaling', text: 'Videos below HD shown larger than they are get scaled up with AMD FSR 1 on the GPU.' },
      { icon: 'mini', title: 'Mini player', text: 'A small, resizable window that stays above other apps. Vertical videos get a vertical window.' },
      { icon: 'seek', title: 'Seek bar', text: 'Previews, chapters, torrent speed and peers.' },
    ],
  },
  {
    name: 'Privacy',
    items: [
      { icon: 'shield', title: 'Shield', text: 'Torrents go through the SOCKS5 proxy of your VPN provider, so peers do not see your address.' },
      { icon: 'trash', title: 'Nothing left on disk', text: 'Torrent data and converted segments are deleted when the app quits.' },
      { icon: 'code', title: 'Free software', text: 'GPL-3.0-or-later. No account, no tracking, no ads.' },
    ],
  },
];
