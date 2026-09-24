Status: approved

# 0021: Artwork framework

## Goal

Give each environment a store of artwork: an icon, a grid (poster), a hero
(banner), and a logo. Art can come from SteamGridDB or from files the user
already has, and manual files never need an API key. This plan builds the
store, the commands, and the SteamGridDB client. Using the stored icon for
desktop entries is plan 0022, and showing posters and heroes is for
`wineo-gui`.

## Existing foundation

- `download.rs` has a blocking `reqwest` client behind a `Transport` trait,
  with a fixture transport for tests, HTTPS only, and a `wineo/<version>` user
  agent. SteamGridDB calls can reuse it.
- Desktop entry icons today come from `winemenubuilder` and are written to
  `hicolor/<size>/apps/wineo-<env>-<target>.png`. This plan does not change
  that.
- There is no image decoding crate in the workspace.

## SteamGridDB facts

- Base URL `https://www.steamgriddb.com/api/v2`, with the key sent as
  `Authorization: Bearer <key>`.
- `GET /search/autocomplete/{term}` returns games with `id`, `name`,
  `release_date`, and `verified`.
- `GET /{icons,grids,heroes,logos}/game/{id}` returns images with `id`,
  `score`, `style`, `url`, `thumb`, `width`, `height`, and `mime`. Filters:
  `styles`, `dimensions`, `mimes`, `types` (static or animated), `nsfw`, and
  `humor`.
- Icons are PNG or `.ico` from 16 to 1024 px, styled `official` or `custom`.
  Grids are PNG, JPEG, or WebP, with 600x900 as the common poster size. Heroes
  are 1920x620 or larger. Logos are PNG or WebP.

## Proposed decisions for maintainer approval

### Store

- Art lives in `~/.local/share/wineo/art/<env>/`, or `DIR/art/<env>/` in a
  portable folder so it moves with the folder. Each kind is one file named for
  its kind and real format: `icon.png`, `icon.svg`, `grid.jpg`, and so on.
- A target can override any kind in `art/<env>/<target>/`. A kind the target
  does not override falls back to the environment's.
- `art/<env>/art.toml` records the SteamGridDB game ID and, for each kind,
  where the file came from: a SteamGridDB asset ID, or `manual` plus the
  original file name. It carries `schema_version = 1` under 0018's rules.
  Environment configs do not change, so this needs no config schema bump.
- `clone` and `portable copy` copy the art directory to the new name.
  `rm` removes it. Snapshots do not include art.

### Manual files

- `wineo art set ENV[.TARGET] KIND FILE` copies the file into the store. It
  accepts PNG, JPEG, and SVG for every kind, plus WebP for grids, heroes, and
  logos. The type comes from the file's contents, not its extension. Anything
  else is refused.
- The original file is kept as is. Converting JPEG to PNG for icon themes is
  plan 0022's concern, so this plan adds no image decoding crate.
- A manual file is never overwritten by `art fetch` unless `--replace` is
  given.

### SteamGridDB key

- Lookup order: `WINEO_STEAMGRIDDB_KEY`, then the system keyring through the
  Secret Service API, then a `0600` file at `~/.config/wineo/steamgriddb.key`.
  The key file always lives in the host XDG config dir, never in a portable
  folder, and is never shown by `wineo config`.
- `wineo art key` prompts for the key without echoing it, checks it against
  the API, and saves it to the keyring. With no keyring it saves the `0600`
  file and says so. `wineo art key --remove` deletes it from both.
- A command that needs the key and finds none says to run `wineo art key` or
  set the variable, in one short line.

### Fetching

- `wineo art fetch ENV[.TARGET]` searches SteamGridDB with the environment
  name, or `--search TERM`. It shows the matches as a numbered list and saves
  the pick in `art.toml`. Later fetches reuse the saved game. `--game ID`
  skips the search for scripts.
- For each kind it downloads the highest-scoring static image with `nsfw` and
  `humor` off. Icons prefer `official` PNG, grids prefer 600x900, and heroes
  prefer 1920x620. `--only KIND` limits the fetch to one kind, and `--asset ID`
  picks a specific image found on the website.
- Downloads are staged and published without replacing a file wineo did not
  write, the same way component downloads are.

### Other commands

- `wineo art list ENV` shows each kind, its source, and whether a target
  override exists.
- `wineo art rm ENV[.TARGET] KIND` removes one piece of art.
- `list`, `status`, and JSON output do not change in this plan.

## Implementation sequence

1. Add the art store: paths for XDG and portable folders, `art.toml` with its
   schema check, content-type detection, and target fallback.
2. Add `art set`, `art list`, and `art rm`, then carry the art directory
   through `clone`, `portable copy`, and `rm`.
3. Add the key lookup and `art key`, with keyring access behind a small trait
   so tests never touch a real keyring.
4. Add the SteamGridDB client on the existing transport, plus `art fetch`
   with search, picking, image selection, and staged publishing.
5. Document the commands and update the feature backlog.

## Verification and acceptance

- Unit tests use the fixture transport and a fake keyring: search results,
  image selection preferences, filters, a key that fails the check, the key
  lookup order, and a missing key.
- `art set` accepts PNG, JPEG, SVG, and WebP by content, refuses a renamed
  text file, and keeps the original bytes. Target overrides fall back
  correctly.
- `art fetch` never replaces a manual file without `--replace`, and an
  interrupted download leaves no partial file.
- Art follows `clone`, `portable copy`, and a moved portable folder, and `rm`
  removes it.
- A real run on this machine: save a key to GNOME Keyring with `art key`,
  fetch art for one real game, and set one manual icon.
- Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo
  test`.

## Scope limits

This plan stores art and does not display it. It does not change desktop
entries or icons (plan 0022), resize or convert images, show thumbnails in the
terminal, or browse images interactively. Animated art is out of scope.
