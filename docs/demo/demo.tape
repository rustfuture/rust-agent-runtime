Output docs/demo/demo.gif
Set Shell "bash"
Set FontSize 16
Set Width 1200
Set Height 470
Set Padding 28
Set Theme "Catppuccin Mocha"
Hide
Type `export PS1='$ ' PATH="$PWD/target/release:$PATH"`
Enter
Type `clear; printf '$ rust-agent-runtime demo\n'; RAR_DEMO_FAST=1 rust-agent-runtime demo`
Enter
Sleep 1s
Show
Sleep 4s
Hide
Type "clear"
Enter
Type "rust-agent-runtime demo"
Show
Sleep 600ms
Enter
Sleep 4s
Sleep 5s
