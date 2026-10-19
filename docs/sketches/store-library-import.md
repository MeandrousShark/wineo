# Store library import: Epic

Show whole store libraries in wineo, installed or not, and launch through
the store's launcher, like Playnite. Backlog row in
[features.md](../features.md): UX, priority 3, depends on wineo-gui. The
row keeps its priority and status; this page keeps the detail.

From the maintainer (2026-10-02). Steam: installed games come from
`libraryfolders.vdf` and each library's `appmanifest_<appid>.acf` (native
`~/.steam/steam` and Flatpak `~/.var/app/com.valvesoftware.Steam`). Owned
but uninstalled games need the Steam Web API's `GetOwnedGames` with a key
kept in the keyring (no keyless route works anymore, see
[0048](../plans/0048-steam-owned-games.md)). Launch with
`steam://rungameid/<appid>` (or `steam -applaunch <appid> <args>` to pass
arguments), install with `steam://install/<appid>` (Steam's own dialog),
and watch the manifest's state and downloaded bytes for install progress.
Steam starts the game, so running status, Stop, and playtime have to find
the `reaper SteamLaunch AppId=<id>` process instead of supervising it. Env
vars and wrappers only reach the game through its Steam launch options
(`VAR=1 %command%` in `localconfig.vdf`, editable only while Steam is
closed).

Changed by the maintainer (2026-10-09): GOG goes direct, without Heroic,
so store libraries don't depend on it. GOG games are DRM-free, so an
installed GOG game can be a normal wineo environment rather than proxied:
log in through GOG's OAuth (the Galaxy client ID that Heroic's gogdl,
Minigalaxy, and lgogdownloader use, which is unofficial), list owned games
from the account endpoints, and install by running the game's offline
installer into a new prefix (native `.sh` installers tie into the native
installers idea). Epic moved to its own backlog row.

Freebies: Steam art from `appcache/librarycache/<appid>/` without a
SteamGridDB key, store playtime to import like 0029 does for Heroic, and
SteamGridDB lookup by app ID.

Settled: a new proxied environment kind, so wineo overrides can apply
where the store allows (no snapshots, winetricks, exec, clone, or portable
copies, since the store owns the prefix); installed games are scanned live
and the owned list is cached and refreshed; uninstalled games show
grayscaled and dimmed. [0047](../plans/0047-steam-library.md) covers Steam
installed games. Needs a plan. Steam is done in
[0047](../plans/0047-steam-library.md) and
[0048](../plans/0048-steam-owned-games.md), GOG in
[0049](../plans/0049-gog-games.md), [0050](../plans/0050-gog-installs.md),
and [0052](../plans/0052-galaxy-builds.md); Epic remains.
