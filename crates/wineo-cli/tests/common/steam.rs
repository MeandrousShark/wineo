use std::fs;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};

use super::{Fixture, wait_for};

const REAPER: &str = r#"
#define _POSIX_C_SOURCE 200809L
#include <fcntl.h>
#include <limits.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static void write_pid_atomic(const char *path, pid_t pid) {
    char temporary[PATH_MAX];
    if (snprintf(temporary, sizeof(temporary), "%s.tmp.%ld", path, (long)getpid()) >= (int)sizeof(temporary)) return;
    FILE *file = fopen(temporary, "w");
    if (!file) return;
    int ok = fprintf(file, "%ld\n", (long)pid) >= 0;
    if (fclose(file)) ok = 0;
    if (!ok || rename(temporary, path)) unlink(temporary);
}

static void append_pid(const char *path, pid_t pid) {
    int fd = open(path, O_WRONLY | O_CREAT | O_APPEND, 0600);
    if (fd < 0) return;
    char line[64];
    int length = snprintf(line, sizeof(line), "%ld\n", (long)pid);
    if (length > 0) (void)write(fd, line, (size_t)length);
    close(fd);
}

int main(int argc, char **argv) {
    if (argc < 5 || strcmp(argv[1], "SteamLaunch") || strncmp(argv[2], "AppId=", 6)) return 2;
    if (!strcmp(argv[4], "exit")) {
        struct timespec delay = {.tv_sec = 1, .tv_nsec = 500000000};
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
        const char *registry = getenv("WINEO_TEST_STEAM_PIDS");
        if (registry) append_pid(registry, child);
        if (argc > 5) {
            write_pid_atomic(argv[5], child);
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

static void append_pid(const char *path, pid_t pid) {
    int fd = open(path, O_WRONLY | O_CREAT | O_APPEND, 0600);
    if (fd < 0) return;
    char line[64];
    int length = snprintf(line, sizeof(line), "%ld\n", (long)pid);
    if (length > 0) (void)write(fd, line, (size_t)length);
    close(fd);
}

int main(int argc, char **argv) {
    if (argc != 5) return 2;
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
            if (child > 0) append_pid(argv[4], child);
        }
        fclose(stream);
    }
}
"#;

const STEAM: &str = r#"
#define _POSIX_C_SOURCE 200809L
#include <fcntl.h>
#include <limits.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <unistd.h>

static void append(const char *path, const char *text) {
    int fd = open(path, O_WRONLY | O_CREAT | O_APPEND, 0600);
    if (fd < 0) return;
    (void)write(fd, text, strlen(text));
    close(fd);
}

int main(int argc, char **argv) {
    const char *root = getenv("WINEO_TEST_STEAM_FIXTURE");
    if (!root) return 3;
    if (argc == 2 && !strncmp(argv[1], "steam://", 8)) {
        char urls[PATH_MAX], line[1024];
        snprintf(urls, sizeof(urls), "%s/steam-urls", root);
        snprintf(line, sizeof(line), "%s\n", argv[1]);
        append(urls, line);
        return 0;
    }
    if (argc < 3 || strcmp(argv[1], "-applaunch")) return 2;
    char dispatches[PATH_MAX], mode_path[PATH_MAX], fifo[PATH_MAX];
    char linger[PATH_MAX], request_pids[PATH_MAX], path[PATH_MAX];
    snprintf(dispatches, sizeof(dispatches), "%s/steam-dispatches", root);
    snprintf(mode_path, sizeof(mode_path), "%s/steam-mode", root);
    snprintf(fifo, sizeof(fifo), "%s/steam-request.fifo", root);
    snprintf(linger, sizeof(linger), "%s/steam-linger", root);
    snprintf(request_pids, sizeof(request_pids), "%s/steam-request-pids", root);
    snprintf(path, sizeof(path), "%ld\n", (long)getpid());
    append(request_pids, path);
    char dispatch[1024] = {0};
    size_t used = 0;
    for (int i = 1; i < argc; i++) {
        int written = snprintf(dispatch + used, sizeof(dispatch) - used, "%s%s", i == 1 ? "" : " ", argv[i]);
        if (written < 0 || (size_t)written >= sizeof(dispatch) - used) return 4;
        used += (size_t)written;
    }
    strncat(dispatch, "\n", sizeof(dispatch) - strlen(dispatch) - 1);
    append(dispatches, dispatch);

    FILE *mode_file = fopen(mode_path, "r");
    char mode[128] = "hold";
    if (mode_file) {
        if (!fgets(mode, sizeof(mode), mode_file)) strcpy(mode, "hold");
        fclose(mode_file);
    }
    mode[strcspn(mode, "\r\n")] = 0;
    if (strcmp(mode, "none")) {
        int fd = open(fifo, O_WRONLY);
        if (fd < 0) return 5;
        char request[256];
        int length = snprintf(request, sizeof(request), "AppId=%s %s\n", argv[2], mode);
        if (length <= 0 || (size_t)length >= sizeof(request) || write(fd, request, (size_t)length) != length) {
            close(fd);
            return 6;
        }
        close(fd);
    }
    if (!access(linger, F_OK)) for (;;) pause();
    return 0;
}
"#;

pub struct SteamFixture {
    pub f: Fixture,
    pub root: PathBuf,
    pub fifo: PathBuf,
    pub dispatches: PathBuf,
    pub mode: PathBuf,
    pub child_pid: PathBuf,
    pub reaper_pids: PathBuf,
    pub request_pids: PathBuf,
    client: Child,
    other_client: Option<Child>,
    other_processes: Vec<Child>,
}

impl SteamFixture {
    pub fn new() -> Self {
        let f = Fixture::new();
        let root = f.root.path().join(".local/share/Steam");
        let reaper_dir = root.join("ubuntu12_32");
        fs::create_dir_all(&reaper_dir).unwrap();
        let reaper = reaper_dir.join("reaper");
        let client_binary = reaper_dir.join("steam");
        compile(&reaper, REAPER);
        compile(&client_binary, CLIENT);
        let fifo = f.root.path().join("steam-request.fifo");
        let dispatches = f.root.path().join("steam-dispatches");
        let mode = f.root.path().join("steam-mode");
        let child_pid = f.root.path().join("steam-child-pid");
        let reaper_pids = f.root.path().join("steam-reaper-pids");
        let request_pids = f.root.path().join("steam-request-pids");
        fs::write(&mode, "hold\n").unwrap();
        fs::write(&dispatches, "").unwrap();
        fs::write(&reaper_pids, "").unwrap();
        fs::write(&request_pids, "").unwrap();
        let steam = f.root.path().join("bin/steam");
        compile(&steam, STEAM);
        let client = Command::new(client_binary)
            .args([&fifo, &reaper, &child_pid, &reaper_pids])
            .env("WINEO_TEST_STEAM_PIDS", &request_pids)
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
            reaper_pids,
            request_pids,
            client,
            other_client: None,
            other_processes: Vec::new(),
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
        self.set_game_progress_in(&self.root, app_id, title, flags, downloaded, staged);
    }

    pub fn add_library(&self, path: &Path) {
        fs::create_dir_all(path.join("steamapps")).unwrap();
        fs::write(
            self.root.join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} \"1\" {{ \"path\" \"{}\" }} }}\n",
                self.root.display(),
                path.display()
            ),
        )
        .unwrap();
    }

    pub fn set_game_progress_in(
        &self,
        library: &Path,
        app_id: &str,
        title: &str,
        flags: u32,
        downloaded: u64,
        staged: u64,
    ) {
        let steamapps = library.join("steamapps");
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
            .env("WINEO_TEST_STEAM_FIXTURE", self.f.root.path())
            .env("WINEO_TEST_STEAM_START_TIMEOUT_MS", "5000")
            .env("WINEO_TEST_STEAM_STALL_TIMEOUT_MS", "5000")
            .env("WINEO_TEST_STEAM_SCAN_INTERVAL_MS", "25");
        command
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.wineo().args(args).output().unwrap()
    }

    pub fn run_with_start_timeout(&self, args: &[&str], timeout_ms: u64) -> Output {
        self.wineo()
            .env("WINEO_TEST_STEAM_START_TIMEOUT_MS", timeout_ms.to_string())
            .args(args)
            .output()
            .unwrap()
    }

    /// The `steam://` URLs the fake Steam was asked to open, in order.
    pub fn urls(&self) -> Vec<String> {
        fs::read_to_string(self.f.root.path().join("steam-urls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
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

    pub fn tracked_pids(&self) -> Vec<u32> {
        let mut pids = vec![self.client.id()];
        if let Some(client) = &self.other_client {
            pids.push(client.id());
        }
        pids.extend(self.other_processes.iter().map(Child::id));
        for path in [&self.reaper_pids, &self.request_pids, &self.child_pid] {
            pids.extend(read_pids(path));
        }
        pids.sort_unstable();
        pids.dedup();
        pids
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
        let client_binary = root.join("ubuntu12_32/steam");
        compile(&client_binary, CLIENT);
        let client = Command::new(client_binary)
            .args([&fifo, &reaper, &self.child_pid, &self.reaper_pids])
            .env("WINEO_TEST_STEAM_PIDS", &self.request_pids)
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

    pub fn launch_from_unrelated_parent(&mut self, app_id: &str) -> u32 {
        let child = Command::new(self.root.join("ubuntu12_32/reaper"))
            .args([
                "SteamLaunch",
                &format!("AppId={app_id}"),
                "--",
                "hold",
                self.child_pid.to_str().unwrap(),
            ])
            .env("WINEO_TEST_STEAM_PIDS", &self.request_pids)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        self.other_processes.push(child);
        pid
    }
}

impl Drop for SteamFixture {
    fn drop(&mut self) {
        let _ = self.run(&["stop", "--force", "civ"]);
        for pid in self.tracked_pids() {
            signal_fixture_process(self.f.root.path(), pid);
        }
        if let Some(client) = &mut self.other_client {
            let _ = client.wait();
        }
        let _ = self.client.wait();
        for child in &mut self.other_processes {
            let _ = child.wait();
        }
    }
}

pub fn read_pid(path: &Path) -> Option<u32> {
    read_pids(path).into_iter().next()
}

fn read_pids(path: &Path) -> Vec<u32> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .filter(|pid| *pid != 0)
        .collect()
}

fn signal_fixture_process(root: &Path, pid: u32) {
    let Some(rustix_pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) else {
        return;
    };
    let Ok(pidfd) = pidfd_open(rustix_pid, PidfdFlags::empty()) else {
        return;
    };
    let Some(start_time) = wineo_core::process::start_time(pid) else {
        return;
    };
    let Ok(exe) = fs::read_link(format!("/proc/{pid}/exe")) else {
        return;
    };
    let Ok(root) = fs::canonicalize(root) else {
        return;
    };
    let Ok(exe) = fs::canonicalize(exe) else {
        return;
    };
    if !exe.starts_with(root) || wineo_core::process::start_time(pid) != Some(start_time) {
        return;
    }
    let mut descriptors = [PollFd::new(&pidfd, PollFlags::IN)];
    if poll(&mut descriptors, Some(&Timespec::default())).is_err()
        || !descriptors[0].revents().is_empty()
    {
        return;
    }
    let _ = pidfd_send_signal(&pidfd, Signal::KILL);
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
