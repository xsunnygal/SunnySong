import { readFile } from "node:fs/promises";

const readJson = async (path) => JSON.parse(await readFile(path, "utf8"));
const packageJson = await readJson("package.json");
const tauriConfig = await readJson("src-tauri/tauri.conf.json");
const expected = packageJson.version;
const manifests = [
  "src-tauri/Cargo.toml",
  "crates/solmusic-application/Cargo.toml",
  "crates/solmusic-domain/Cargo.toml",
  "crates/solmusic-sqlite/Cargo.toml",
  "crates/solmusic-youtube/Cargo.toml",
  "crates/tauri-plugin-solmusic-storage/Cargo.toml",
];
const mismatches = [];
if (tauriConfig.version !== expected) mismatches.push(`src-tauri/tauri.conf.json=${tauriConfig.version}`);
for (const path of manifests) {
  const manifest = await readFile(path, "utf8");
  const version = manifest.match(/^version = "([^"]+)"/m)?.[1];
  if (version !== expected) mismatches.push(`${path}=${version ?? "missing"}`);
}
const about = await readFile("src/routes/about/+page.svelte", "utf8");
if (!about.includes(`<h2>${expected}</h2>`)) mismatches.push("About changelog has no current-version entry");
if (mismatches.length) {
  throw new Error(`Version ${expected} is not synchronized: ${mismatches.join(", ")}`);
}
console.log(`Version ${expected} is synchronized.`);
