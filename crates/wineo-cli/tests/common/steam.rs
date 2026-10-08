use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use super::{Fixture, wait_for};

const REAPER: &str = r#"
#define _POSIX_C_SOURCE 200809L
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 5 || strcmp(argv[1], "SteamLaunch") || strncmp(argv[2], "AppId=", 6)) return 2;
    if (!strcmp(argv[4], "exit")) {
        struct timespec delay = {.tv_sec = 0, .tv_nsec = 300000000};
        nanosleep(&delay, NULL);
        return 0;
    }
    if (!strcmp(argv[4], "child")) {
        pid_t child = fork();
        if (child == 0) {
            strcpy(argv[1], "other");
            signal(SIGTERM, SIG_IGN);
            for (;;) pause();
        }
        if (child < 0) return 3;
        if (argc > 5) {
            FILE *file = fopen(argv[5], "w");
            if (file) { fprintf(file, "%ld\n", (long)child); fclose(file); }
        }
    }
    for (;;) pause();
}
"#;

const CLIENT: &str = r#"
#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

static void reap(int signal_number) {
    (void)signal_number;
    while (waitpid(-1, NULL, WNOHANG) > 0) {}
}

int main(int argc, char **argv) {
    if (argc != 4) return 2;
    struct sigaction action = {0};
    action.sa_handler = reap;
    sigaction(SIGCHLD, &action, NULL);
    if (mkfifo(argv[1], 0600) && errno != EEXIST) return 3;
    for (;;) {
        int input = open(argv[1], O_RDONLY);
        if (input < 0) { if (errno == EINTR) continue; return 4; }
        FILE *stream = fdopen(input, "r");
        if (!stream) { close(input); return 5; }
        char request[256];
        while (fgets(request, sizeof(request), stream)) {
            char *mode = strchr(request, ' ');
            if (!mode) continue;
            *mode++ = 0;
            mode[strcspn(mode, "\n")] = 0;
            if (!strcmp(mode, "none")) continue;
            pid_t child = fork();
            if (child == 0) execl(argv[2], argv[2], "SteamLaunch", request, "--", mode, argv[3], (char *)NULL);
            if (child == 0) _exit(127);
        }
        fclose(stream);
    }
}
"#;

pub struct SteamFixture {
    pub f: Fixture,
    pub root: PathBuf,
    pub fifo: PathBuf,
    pub dispatches: PathBuf,
    pub mode: PathBuf,
    pub child_pid: PathBuf,
    client: Child,
    other_client: Option<Child>,
}

impl SteamFixture {
    pub fn new() -> Self {
        let f = Fixture::new();
        let root = f.root.path().join(".local/share/Steam");
        let reaper_dir = root.join("ubuntu12_32");
        fs::create_dir_all(&reaper_dir).unwrap();
        let reaper = reaper_dir.join("reaper");
        let client_binary = f.root.path().join("bin/fake-steam-client");
        compile(&reaper, REAPER);
        compile(&client_binary, CLIENT);
        let fifo = f.root.path().join("steam-request.fifo");
        let dispatches = f.root.path().join("steam-dispatches");
        let mode = f.root.path().join("steam-mode");
        let child_pid = f.root.path().join("steam-child-pid");
        fs::write(&mode, "hold\n").unwrap();
        fs::write(&dispatches, "").unwrap();
        let steam = f.root.path().join("bin/steam");
        fs::write(
            &steam,
            format!(
                "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >> '{}'\nIFS= read -r mode < '{}'\nprintf 'AppId=%s %s\\n' \"$2\" \"$mode\" > '{}'\nif [ -e '{}' ]; then while :; do sleep 1; done; fi\n",
                dispatches.display(),
                mode.display(),
                fifo.display(),
                f.root.path().join("steam-linger").display(),
            ),
        )
        .unwrap();
        fs::set_permissions(&steam, fs::Permissions::from_mode(0o755)).unwrap();
        let client = Command::new(client_binary)
            .args([&fifo, &reaper, &child_pid])
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_for("fake Steam client FIFO", || fifo.exists());
        let fixture = Self {
            f,
            root,
            fifo,
            dispatches,
            mode,
            child_pid,
            client,
            other_client: None,
        };
        fixture.add_game("289070", "Civilization VI", 4);
        fixture
    }

    pub fn add_game(&self, app_id: &str, title: &str, flags: u32) {
        self.set_game_progress(app_id, title, flags, 0, 0);
    }

    pub fn set_game_progress(
        &self,
        app_id: &str,
        title: &str,
        flags: u32,
        downloaded: u64,
        staged: u64,
    ) {
        let steamapps = self.root.join("steamapps");
        fs::create_dir_all(&steamapps).unwrap();
        let manifest = steamapps.join(format!("appmanifest_{app_id}.acf"));
        let temporary = steamapps.join(format!("appmanifest_{app_id}.tmp"));
        fs::write(
            &temporary,
            format!("\"AppState\"\n{{\n\"appid\" \"{app_id}\"\n\"name\" \"{title}\"\n\"StateFlags\" \"{flags}\"\n\"BytesDownloaded\" \"{downloaded}\"\n\"BytesStaged\" \"{staged}\"\n}}\n"),
        ).unwrap();
        fs::rename(temporary, manifest).unwrap();
    }

    pub fn config(&self, hooks: &str) {
        let dir = self.f.config_dir().join("environments");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("civ.toml"),
            format!("kind = \"proxied\"\nlauncher = \"steam\"\napp_id = \"289070\"\ntitle = \"Civilization VI\"\n{hooks}"),
        )
        .unwrap();
    }

    pub fn wineo(&self) -> Command {
        let mut command = self.f.wineo_at_home();
        command
            .env("WINEO_TEST_STEAM_START_TIMEOUT_MS", "450")
            .env("WINEO_TEST_STEAM_STALL_TIMEOUT_MS", "450");
        command
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.wineo().args(args).output().unwrap()
    }

    pub fn dispatch_count(&self) -> usize {
        fs::read_to_string(&self.dispatches)
            .unwrap()
            .lines()
            .count()
    }

    pub fn launch_from_steam(&self, app_id: &str) {
        let mut output = fs::OpenOptions::new().write(true).open(&self.fifo).unwrap();
        writeln!(output, "AppId={app_id} hold").unwrap();
    }

    pub fn client_pid(&self) -> u32 {
        self.client.id()
    }

    pub fn set_mode(&self, mode: &str) {
        fs::write(&self.mode, format!("{mode}\n")).unwrap();
    }

    pub fn launch_from_other_install(&mut self, app_id: &str) {
        let root = self
            .f
            .root
            .path()
            .join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
        let reaper = root.join("ubuntu12_32/reaper");
        fs::create_dir_all(reaper.parent().unwrap()).unwrap();
        compile(&reaper, REAPER);
        let fifo = self.f.root.path().join("other-steam-request.fifo");
        let client = Command::new(self.f.root.path().join("bin/fake-steam-client"))
            .args([&fifo, &reaper, &self.child_pid])
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_for("other fake Steam client FIFO", || fifo.exists());
        let mut output = fs::OpenOptions::new().write(true).open(fifo).unwrap();
        writeln!(output, "AppId={app_id} hold").unwrap();
        self.other_client = Some(client);
    }
}

impl Drop for SteamFixture {
    fn drop(&mut self) {
        let _ = self.run(&["stop", "--force", "civ"]);
        kill_group(self.client.id());
        if let Some(client) = &mut self.other_client {
            kill_group(client.id());
            let _ = client.wait();
        }
        let _ = self.client.wait();
    }
}

fn kill_group(pid: u32) {
    let _ = Command::new("kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .status();
}

fn compile(output: &Path, source: &str) {
    let source_file = output.with_extension("c");
    fs::write(&source_file, source).unwrap();
    let result = Command::new("cc")
        .args(["-std=c11", "-Wall", "-Wextra", "-O0", "-o"])
        .arg(output)
        .arg(&source_file)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
