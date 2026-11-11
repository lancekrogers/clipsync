# The terminal companion against an isolated fixture. Run: just demo-tui
# Settings is never opened (no 4, no Tab): it reads and controls the real
# background service. Esc outside search quits, so search ends with Enter.
Output docs/assets/companion-tui.gif

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
Sleep 3s

Type "2"
Wait+Screen@10s /Authorized devices/
Sleep 3s

# Enter selects the newest copy, an image previewed in colored characters.
Type "3"
Wait+Screen@10s /Lunch at 12:30/
Sleep 1.5s
Enter
Sleep 4s

# Copy a line that arrived from the other device.
Down@500ms 7
Sleep 2s
Type "c"
Wait+Screen@10s /Copied/
Sleep 3s

Type "/"
Sleep 500ms
Type "lunch"
Sleep 2.5s
Backspace@60ms 5
Enter
Sleep 1.5s

Type "p"
Wait+Screen@10s /Paused/
Sleep 3s
Type "p"
Sleep 2s

Type "1"
Sleep 2.5s

Hide
Type "q"
Sleep 1s
Type "demo_stop" Enter
Sleep 2s
