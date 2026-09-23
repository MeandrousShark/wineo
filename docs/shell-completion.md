# Shell completion

Wineo provides dynamic completion for Bash, Zsh, and Fish. The generated
initialization snippet asks the installed `wineo` binary for its current
completion registration when the shell loads it. Add the Bash or Zsh output
to the corresponding startup file once:

```sh
wineo completions bash >> ~/.bashrc
wineo completions zsh >> ~/.zshrc
```

For Fish, save the output as a completion file:

```fish
wineo completions fish > ~/.config/fish/completions/wineo.fish
```

Completion offers configured environment names in the environment arguments
for `setup`, `config`, `start`, `stop`, `restart`, `programs`, `rm`, `status`,
`logs`, `run`, and `exec`. It also offers the environment argument for `set`
unless `--global` is present, and the first environment value passed to
`winetricks`. For example, `wineo start <Tab>` offers environments.

For `start`, `run`, and `restart`, `--target` offers targets from the selected
environment only:

```sh
wineo run hades --target <Tab>
```

Wineo reads configuration to build candidates. It does not launch Wine or
modify files. If configuration cannot be read, completion quietly returns no
candidates. A broken environment config can still be completed by name, but
its targets cannot be loaded.

Elvish and PowerShell continue to receive static command and flag scripts from
`wineo completions elvish` and `wineo completions powershell`. These scripts
do not include live environment or target names.
