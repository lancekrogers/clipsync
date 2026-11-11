# Delete one saved copy, then clear all history, in the terminal companion
# against the isolated fixture. Run: just demo-tui-history-remove
# Settings is never opened (no 4, no Tab): it reads and controls the real
# background service.
Output docs/assets/companion-tui-history-remove.gif

Set Shell "bash"
Set FontFamily "JetBrainsMono Nerd Font"
Set FontSize 16
Set Width 1200
Set Height 720
Set Padding 20
Set Framerate 24
Set TypingSpeed 75ms
Set Theme {"name":"clipsync","background":"#12141a","foreground":"#d7dae0","cursor":"#69bfff","selection":"#2a2f3a","black":"#1d2129","red":"#e06c75","green":"#98c379","yellow":"#e5c07b","blue":"#61afef","magenta":"#c678dd","cyan":"#56b6c2","white":"#c8ccd4","brightBlack":"#5c6370","brightRed":"#ff7a85","brightGreen":"#b5e890","brightYellow":"#ffd68a","brightBlue":"#7cc4ff","brightMagenta":"#de99f0","brightCyan":"#6fd3df","brightWhite":"#ffffff"}

Hide
Type "source demos/companion-fixture.sh && clear" Enter
Sleep 3s
Show

Type "clipsync tui"
Sleep 400ms
Enter
Wait+Screen@20s /Connected · workbench/
Sleep 1.5s

Type "3"
Wait+Screen@10s /Lunch at 12:30/
Sleep 1.5s

# Rows: the image, five local copies, then three from other devices. Select
# the line copied on workbench.
Enter
Sleep 800ms
Down@300ms 7
Sleep 2.5s

Type "d"
Wait+Screen@5s /Delete this saved copy/
Sleep 3s
Type "y"
Wait+Screen@10s /Deleted/
Sleep 3.5s

Type "D"
Wait+Screen@5s /Clear all history/
Sleep 3s
Type "y"
Wait+Screen@10s /Cleared/
Sleep 4s

Hide
Type "q"
Sleep 1s
Type "demo_stop" Enter
Sleep 2s
