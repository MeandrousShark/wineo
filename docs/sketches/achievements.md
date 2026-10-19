# Achievements

Earn and see achievements for GOG and emulated games outside Steam.
Backlog row in [features.md](../features.md): UX, priority 4, depends on
0052, emulated games. The row keeps its priority and status; this page
keeps the detail.

From the maintainer (2026-10-14), parked until it seems worth the trouble.
Two integrations are enough: GOG through
[comet](https://github.com/imLinguin/comet), an open-source stand-in for
Galaxy's communication service that runs beside a Windows game under Wine
or Proton and records real unlocks on the GOG account (GOG's Linux builds
don't use the Galaxy SDK, so they have none), and RetroAchievements, which
emulators like RetroArch, Dolphin, PCSX2, and DuckStation already support
once logged in, with wineo showing progress through its web API.

Check comet's status first: its first release was July 2024, with Heroic
and Lutris support then planned. Not planned: manual checklists borrowed
from a store's list, like Playnite's SuccessStory offers.
