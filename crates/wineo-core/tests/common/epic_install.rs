//! What the fake `legendary` does for `info`, `install`, and `uninstall`,
//! shared by core's fake and the CLI's through `#[path]`. Each install is
//! told how to behave by a file the test writes, `<root>/mode-<app>`:
//!
//! - none or empty: progress lines, then the game's files and its entry in
//!   `installed.json`, as Legendary 0.21.1 leaves them.
//! - `hang`: starts downloading, then waits up to 30 seconds. An interrupt
//!   writes the resume file, logs Legendary's abort line, and exits 0, as
//!   Legendary does.
//! - `deaf`: like `hang`, but ignores interrupts in its whole group and
//!   holds `/dev/shm/psm_<tag>_held` open, `<tag>` read from
//!   `<root>/shm-tag`, as a killed downloader leaves its shared memory.
//! - `unrecorded`: exits 0 having recorded nothing.
//! - `failed-write`: logs a failed write and records the game anyway.
//! - `elsewhere`: records the game in another folder.
//! - `exit1`: logs an error and exits 1.
//! - `quiet`: starts downloading, prints nothing for 3 seconds, then
//!   finishes.
//! - `outside`: records a program outside the game folder.
//!
//! Every install writes `<root>/installing-<app>` once it has started, and
//! `<root>/got-int` gets a line for each interrupt it heeded.

use std::path::Path;

/// Shell functions the arms use, to put before the `case`.
pub fn functions(tool: &dyn Fn(&str) -> String) -> String {
    let (cat, mv) = (tool("cat"), tool("mv"));
    format!(
        "rebuild() {{\n\
           d=\"$LEGENDARY_CONFIG_PATH/fake-installed\"; out=\"$LEGENDARY_CONFIG_PATH/installed.json\"\n\
           sep=''\n\
           {{ printf '{{'; for f in \"$d\"/*.json; do [ -f \"$f\" ] || continue; printf '%s' \"$sep\"; {cat} \"$f\"; sep=','; done; printf '}}'; }} > \"$out.tmp\"\n\
           {mv} \"$out.tmp\" \"$out\"\n\
         }}\n"
    )
}

/// The `case "$1"` arms for `info`, `install`, and `uninstall`. `fixtures`
/// is the folder of Legendary's scrubbed output.
pub fn arms(fixtures: &Path, tool: &dyn Fn(&str) -> String) -> String {
    let (cat, mkdir, rm, sleep) = (tool("cat"), tool("mkdir"), tool("rm"), tool("sleep"));
    let fixture = |name: &str| fixtures.join(name).display().to_string();
    format!(
        "info)\n\
           if [ ! -f \"$user\" ]; then {cat} '{logged_out}' >&2; exit 1; fi\n\
           case \"$2\" in\n\
           AppOffline) {cat} '{offline}' ;;\n\
           'Dlc Game') {cat} '{dlc}' ;;\n\
           AppPrereq) {cat} '{prereq}' ;;\n\
           AppToken) {cat} '{token}' ;;\n\
           AppThirdParty) {cat} '{third_party}' ;;\n\
           *) echo \"[cli] ERROR: Game not found: $2\" >&2 ;;\n\
           esac ;;\n\
         install)\n\
           app=\"$2\"; shift 2; base=''; folder=''\n\
           while [ $# -gt 0 ]; do\n\
             case \"$1\" in --base-path) base=\"$2\"; shift ;; --game-folder) folder=\"$2\"; shift ;; esac\n\
             shift\n\
           done\n\
           mode=$({cat} \"$root/mode-$app\" 2>/dev/null)\n\
           if [ ! -f \"$user\" ]; then {cat} '{logged_out}' >&2; exit 1; fi\n\
           echo \"[cli] INFO: Preparing download for \\\"$app\\\" ($app)...\" >&2\n\
           echo \"[DLManager] INFO: Download Manager running with process-id: $$\" >&2\n\
           : > \"$root/installing-$app\"\n\
           case \"$mode\" in\n\
           hang)\n\
             {mkdir} -p \"$LEGENDARY_CONFIG_PATH/tmp\"\n\
             trap 'echo \"[cli] INFO: Command was aborted via KeyboardInterrupt, cleaning up...\" >&2; : > \"$LEGENDARY_CONFIG_PATH/tmp/$app.resume\"; echo \"$app\" >> \"$root/got-int\"; exit 0' INT\n\
             echo '[DLManager] INFO: = Progress: 10.00% (1/10), Running for 00:00:01, ETA: 00:00:09' >&2\n\
             {sleep} 30; exit 0 ;;\n\
           deaf)\n\
             trap '' INT\n\
             exec 3>\"/dev/shm/psm_$({cat} \"$root/shm-tag\")_held\"\n\
             echo '[DLManager] INFO: = Progress: 10.00% (1/10), Running for 00:00:01, ETA: 00:00:09' >&2\n\
             {sleep} 30; exit 0 ;;\n\
           exit1) echo '[cli] ERROR: Something went wrong' >&2; exit 1 ;;\n\
           unrecorded) echo '[DLManager] INFO: = Progress: 100.00% (2/2), Running for 00:00:01, ETA: 00:00:00' >&2; exit 0 ;;\n\
           quiet) {sleep} 3 ;;\n\
           esac\n\
           echo '[DLManager] INFO: = Progress: 50.00% (1/2), Running for 00:00:01, ETA: 00:00:01' >&2\n\
           echo '[DLManager] INFO:  - Downloaded: 1.00 MiB, Written: 1.00 MiB' >&2\n\
           echo '[DLManager] INFO: = Progress: 100.00% (2/2), Running for 00:00:02, ETA: 00:00:00' >&2\n\
           {mkdir} -p \"$base/$folder/Binaries/Win64\"\n\
           : > \"$base/$folder/Binaries/Win64/Game.exe\"\n\
           : > \"$base/$folder/installed-$app\"\n\
           if [ \"$mode\" = failed-write ]; then echo '[FileWorker] ERROR: Writing file failed: [Errno 28] No space left on device' >&2; fi\n\
           path=\"$base/$folder\"; if [ \"$mode\" = elsewhere ]; then path=/elsewhere; fi\n\
           {mkdir} -p \"$LEGENDARY_CONFIG_PATH/fake-installed\"\n\
           exe=Binaries/Win64/Game.exe; if [ \"$mode\" = outside ]; then exe=Binaries/../../outside.exe; fi\n\
           printf '\"%s\": {{\"app_name\": \"%s\", \"install_path\": \"%s\", \"version\": \"1.0\", \"executable\": \"%s\", \"launch_parameters\": \"-skipintro -lang=en\", \"can_run_offline\": true}}' \"$app\" \"$app\" \"$path\" \"$exe\" > \"$LEGENDARY_CONFIG_PATH/fake-installed/$app.json\"\n\
           rebuild\n\
           {rm} -f \"$LEGENDARY_CONFIG_PATH/tmp/$app.resume\"\n\
           echo '[cli] INFO: Finished installation process in 0.01 seconds.' >&2 ;;\n\
         uninstall)\n\
           {rm} -f \"$LEGENDARY_CONFIG_PATH/fake-installed/$2.json\"\n\
           rebuild ;;\n",
        logged_out = fixture("logged-out-list.stderr"),
        offline = fixture("info-offline.json"),
        dlc = fixture("info-dlc.json"),
        prereq = fixture("info-prereq.json"),
        token = fixture("info-token.json"),
        third_party = fixture("info-third-party.json"),
    )
}
