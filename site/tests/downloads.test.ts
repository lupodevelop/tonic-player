import assert from 'node:assert/strict';
import { test } from 'node:test';
import { pickDownloads } from '../src/scripts/downloads.ts';

const asset = (name: string) => ({ name, browser_download_url: `https://example.test/${name}` });

// The names tauri-action gives the files of a release.
const release = [
  'Tonic.Player_0.2.0_aarch64.dmg',
  'Tonic.Player_0.2.0_x64.dmg',
  'Tonic.Player_aarch64.app.tar.gz',
  'Tonic.Player_aarch64.app.tar.gz.sig',
  'Tonic.Player_x64.app.tar.gz',
  'Tonic.Player_0.2.0_x64-setup.exe',
  'Tonic.Player_0.2.0_x64-setup.exe.sig',
  'Tonic.Player_0.2.0_x64_en-US.msi',
  'Tonic.Player_0.2.0_amd64.AppImage',
  'Tonic.Player_0.2.0_amd64.AppImage.sig',
  'Tonic.Player_0.2.0_amd64.deb',
  'Tonic.Player-0.2.0-1.x86_64.rpm',
  'latest.json',
].map(asset);

test('each system gets its installer', () => {
  const links = pickDownloads(release);
  assert.equal(links.mac, 'https://example.test/Tonic.Player_0.2.0_aarch64.dmg');
  assert.equal(links.macIntel, 'https://example.test/Tonic.Player_0.2.0_x64.dmg');
  assert.equal(links.win, 'https://example.test/Tonic.Player_0.2.0_x64-setup.exe');
  assert.equal(links.linux, 'https://example.test/Tonic.Player_0.2.0_amd64.AppImage');
});

test('a missing file leaves that system without a link', () => {
  const links = pickDownloads(release.filter((a) => !a.name.endsWith('.exe')));
  assert.equal(links.win, undefined);
  assert.equal(links.mac, 'https://example.test/Tonic.Player_0.2.0_aarch64.dmg');
  assert.deepEqual(pickDownloads([]), {});
});
