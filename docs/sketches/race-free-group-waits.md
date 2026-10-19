# Race-free process group waits

Stop installer runs, hooks, and Proton run settling from calling a process
group empty while a member is still running. Backlog row in
[features.md](../features.md): Core, priority 4. The row keeps its
priority and status; this page keeps the detail.

From 0049 step 4 (2026-10-11). `process::group_has_live_members` lists
`/proc` and then reads each entry, so a member that forks and exits
between the two hides its new child, and a thread leader that exits while
other threads run looks like a zombie. Only tight fork loops hit it. Exec
moved to wineserver's idle wait instead. A real fix likely needs cgroups
or pidfd tracking. The maintainer parked it rather than patching.

Since the fixes after 0.35.0, every Proton run settles through this scan,
waiting for umu's group to empty instead of ending when umu exits. A
member that leaves the group with `setsid` or `setpgid` isn't tracked
either, and the real Steam Linux Runtime (pressure-vessel) group layout is
untested; cgroups would cover both.
