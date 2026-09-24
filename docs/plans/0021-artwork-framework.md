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
- `Paths` resolves `$XDG_CONFIG_HOME`, `$XDG_DATA_HOME`, and
  `$XDG_STATE_HOME`, falling back to `~/.config`, `~/.local/share`, and
  `~/.local/state` when a variable is unset or relative. In a portable folder
  only `host_state` still points at the host.

## SteamGridDB facts

- Base URL `https://www.steamgriddb.com/api/v2`, with the key sent as
  `Authorization: Bearer <key>`.
- `GET /search/autocomplete/{term}` returns games with `id`, `name`,
  `release_date`, and `verified`.
- `GET /{icons,grids,heroes,logos}/game/{id}` returns images with `id`,
  `score`, `style`, `language`, `url`, `thumb`, `width`, `height`, and `mime`.
  Filters: `styles`, `dimensions`, `mimes`, `types` (static or animated),
  `nsfw`, and `humor`. There is no language filter. Results are paginated
  with a `page` parameter.
- The official Node wrapper's types do not match the API exactly (it types
  `style` as an array and leaves out `mime`), so no field beyond the IDs, the
  name, and the URL is treated as guaranteed.
- Icons are PNG or ICO from 16 to 1024 px, styled `official` or `custom`.
  Grids are PNG, JPEG, or WebP, and include horizontal capsules (460x215,
  920x430) and squares as well as portrait posters (600x900, 342x482,
  660x930). Heroes are 1920x620 or larger. Logos are PNG or WebP, styled
  `official`, `white`, `black`, or `custom`.

## Proposed decisions for maintainer approval

### Store

- Art lives in `$XDG_DATA_HOME/wineo/art/<env>/`, or `DIR/art/<env>/` in a
  portable folder so it moves with the folder. Each kind is one file named for
  its kind and real format: `icon.png`, `icon.ico`, `icon.svg`, `grid.jpg`,
  and so on.
- A target can override any kind in `art/<env>/<target>/`. A kind the target
  does not override falls back to the environment's.
- `art/<env>/art.toml` records the SteamGridDB game ID and, for each kind,
  where the file came from: a SteamGridDB asset ID, or `manual` plus the
  original file name. It carries `schema_version = 1` under 0018's rules.
  Environment configs do not change, so this needs no config schema bump.
- `art.toml` layout: a top-level `game` holds the SteamGridDB game ID. Each
  kind is a table with `file`, the stored file name, and exactly one source
  key: `steamgriddb` with the asset ID, or `manual` with the original file
  name. Target overrides live in the same file as `[targets.<target>.<kind>]`
  with the same fields, so the game ID is shared. Unknown keys are refused,
  like other configs, and a bad value makes the whole file invalid. wineo
  edits `art.toml` with `toml_edit`, so hand-added comments survive.
- `art list` warns in one line about a record whose file is missing and about
  a `[targets.<target>]` table for a target the environment does not have. It
  does not change the file. The next `art set` or `art rm` of that kind
  replaces or drops the stale record.

  ```toml
  schema_version = 1
  game = 5247

  [icon]
  file = "icon.png"
  steamgriddb = 36952

  [grid]
  file = "grid.jpg"
  manual = "poster final.jpg"

  [targets.server.icon]
  file = "icon.svg"
  manual = "server.svg"
  ```
- An art file that `art.toml` does not record, including every file when
  `art.toml` is absent, is treated as manual with an unknown origin. `art
  list` shows it as unrecorded, and `art fetch` does not replace it without
  `--replace`.
- If more than one file exists for a kind, the one `art.toml` records wins.
  With no record, the first in the order PNG, SVG, ICO, JPEG, WebP wins, and
  `art list` warns about the others. `art set` and `art fetch` remove the old
  file when a kind changes format.
- `clone` and `portable copy` copy the art directory to the new name.
  `rm` removes it. Snapshots do not include art. When the environment has
  art, `rm` prints `Removed art` and the clone and copy reviews show `art:
  copied`. An environment without art gets no extra line.
- Removing a target also removes its art directory and its records in
  `art.toml`, so a later target with the same name starts clean. A snapshot
  restore that drops a target keeps its art, since snapshots do not include
  art and a later restore may bring the target back. `art list` warns about it.

### Manual files

- `wineo art set ENV[.TARGET] KIND FILE` copies the file into the store. It
  accepts PNG, JPEG, and SVG for every kind, ICO for icons, and WebP for
  grids, heroes, and logos. The type comes from the file's contents, not its
  extension. Anything else is refused, as is any file over 32 MiB.
- ICO needs a nonzero image count and a directory that fits inside the file,
  since its four magic bytes alone are weak.
- SVG has no magic bytes. After an optional BOM, whitespace, XML declaration,
  comments, and doctype, the first element must be `<svg`. This is a sniff,
  not a parse, so it adds no XML crate. A full well-formedness check with
  `roxmltree` can replace it later if bad files turn up.
- The original file is kept as is. Converting JPEG and ICO to PNG for icon
  themes is plan 0022's concern, so this plan adds no image decoding crate.

### SteamGridDB key

- Lookup order: `WINEO_STEAMGRIDDB_KEY`, then the system keyring through the
  Secret Service API, then a `0600` file at
  `$XDG_DATA_HOME/wineo/steamgriddb.key`. The file sits in the data dir
  rather than the config dir because config dirs are often published as
  dotfiles. It always lives in the host data dir, never in a portable folder,
  so `Paths` gains a `host_data` path.
- `wineo art key` prompts for the key without echoing it, checks it against
  the API, and saves it to the keyring. With no keyring it saves the `0600`
  file and warns in one line that the key is stored in plain text, naming the
  file. `wineo art key --remove` deletes it from both.
- A command that needs the key and finds none says to run `wineo art key` or
  set the variable, in one short line.

### Fetching

- `wineo art fetch ENV[.TARGET]` searches SteamGridDB with the environment
  name, or `--search TERM`. It shows the matches as a numbered list, with the
  release year when the result has one, and saves the pick in `art.toml`.
  Later fetches reuse the saved game. `--game ID` skips the search for
  scripts.
- Responses are parsed leniently: a game needs only `id` and `name`, and an
  image only `id` and `url`. Anything else missing counts as unknown. The
  format of a downloaded file comes from its bytes, like manual files, not
  from `mime`.
- Every request filters to static images with `nsfw` and `humor` off. These
  are hard filters, so fetch never falls back to animated, NSFW, or humor
  art. Grid requests also filter `dimensions` to the portrait sizes 600x900,
  342x482, and 660x930.
- Fetch walks result pages until one comes back empty, up to 10 pages, and
  ranks everything it got. Each kind sorts by these keys in order, where
  "largest" is width times height and the lowest asset ID makes the order
  total:
  - icon: at least 128 px on its short side, then `official`, then the user's
    language, then PNG over ICO, then highest score, then largest, then lowest
    ID
  - grid: user's language, then highest score, then largest, then lowest ID
  - hero: highest score, then largest, then lowest ID
  - logo: `official`, then the user's language, then PNG over WebP, then
    highest score, then largest, then lowest ID
- The user's language is the two-letter code from the first of `LC_ALL`,
  `LC_MESSAGES`, and `LANG` that is set, with `C` and `POSIX` meaning `en`. An
  image in that language ranks first, then `en`, then any other or none.
- Fetch fills gaps. A kind that already has art is skipped with a short note,
  and `--replace` re-ranks and replaces every kind it fetches, manual files
  included. `--only KIND` limits the fetch to one kind. `--asset ID` picks a
  specific image found on the website and replaces whatever that kind has,
  since the user named it.
- Downloads are staged and published without replacing a file wineo did not
  write, the same way component downloads are. A download over 32 MiB is
  abandoned.

### Other commands

- `wineo art list ENV` shows the environment's kinds, then a block for each
  target that overrides something. Warnings go to stderr.

  ```
  hades (SteamGridDB game 5247)
    icon  icon.png   SteamGridDB 36952
    grid  grid.jpg   manual, from poster final.jpg
    hero  hero.webp  unrecorded
    logo  none
  hades.server
    icon  icon.svg   manual, from server.svg
  warning: grid.png ignored, grid.jpg is recorded
  ```

  The other warnings read `warning: server/hero.jpg ignored, server/hero.png
  takes precedence`, `warning: server/logo.png is missing`, and `warning: art
  for unknown target `old``. A target's files are shown relative to the
  environment's art directory.
- `wineo art rm ENV[.TARGET] KIND` removes one piece of art.
- `art set` and `art rm` print one line on success, such as `Set icon for
  hades.server` or `Removed grid from hades`.
- `list`, `status`, and JSON output do not change in this plan.

## Implementation sequence

1. Add the art store: paths for XDG and portable folders, `art.toml` with its
   schema check, content-type detection, unrecorded and duplicate files, and
   target fallback.
2. Add `art set`, `art list`, and `art rm`, then carry the art directory
   through `clone`, `portable copy`, and `rm`.
3. Add `host_data`, the key lookup, and `art key`, with keyring access behind
   a small trait so tests never touch a real keyring.
4. Add the SteamGridDB client on the existing transport, plus `art fetch`
   with search, picking, paging, ranking, and staged publishing.
5. Document the commands and update the feature backlog.

## Verification and acceptance

- Unit tests use the fixture transport and a fake keyring: search results
  with and without `release_date`, each kind's ranking order including the
  icon size floor and ties broken by ID, language fallback, filters, paging,
  a key that fails the check, the key lookup order, the plain text warning,
  and a missing key.
- `art set` accepts PNG, JPEG, SVG, and WebP by content, and ICO only for
  icons. It refuses a renamed text file, a truncated ICO, an XML file whose
  root is not `svg`, and a file over the size cap, and keeps the original
  bytes. Target overrides fall back correctly.
- An unrecorded file, including one with `art.toml` absent, is listed as
  unrecorded and survives `art fetch`. Duplicate files for a kind resolve in
  the stated order.
- `art fetch` skips kinds that have art, never replaces a manual file without
  `--replace` or `--asset`, and an interrupted download leaves no partial
  file.
- Art follows `clone`, `portable copy`, and a moved portable folder, and `rm`
  removes it. In a portable folder the key file is still the host's.
- A real run on this machine: save a key to GNOME Keyring with `art key`,
  fetch art for one real game, and set one manual icon.
- Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo
  test`.

## Scope limits

This plan stores art and does not display it. It does not change desktop
entries or icons (plan 0022), resize or convert images, show thumbnails in the
terminal, or browse images interactively. Animated art is out of scope.
