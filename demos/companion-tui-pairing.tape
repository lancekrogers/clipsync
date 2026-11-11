# Pairing two computers with a code, both terminal companions side by side
# against isolated fixtures. Run: just demo-tui-pairing
# Settings is never opened (no 4, no Tab): it reads and controls the real
# background service. The code is a throwaway fixture code.
Output docs/assets/companion-tui-pairing.gif

Set Shell "bash"
Set FontFamily "JetBrainsMono Nerd Font"
Set FontSize 15
Set Width 1800
Set Height 840
Set Padding 16
Set Framerate 24
Set TypingSpeed 90ms
Set Theme {"name":"clipsync","background":"#12141a","foreground":"#d7dae0","cursor":"#69bfff","selection":"#2a2f3a","black":"#1d2129","red":"#e06c75","green":"#98c379","yellow":"#e5c07b","blue":"#61afef","magenta":"#c678dd","cyan":"#56b6c2","white":"#c8ccd4","brightBlack":"#5c6370","brightRed":"#ff7a85","brightGreen":"#b5e890","brightYellow":"#ffd68a","brightBlue":"#7cc4ff","brightMagenta":"#de99f0","brightCyan":"#6fd3df","brightWhite":"#ffffff"}

Hide
Type "source demos/companion-pairing.sh && clear" Enter
Sleep 4s
Type "pair_demo" Enter
Wait+Screen@20s /Travel laptop · joins/
Wait+Screen@20s /(?s)Your clipboard, everywhere.*Your clipboard, everywhere/
Sleep 1s
Show
Sleep 1.5s

# Studio Mac: Devices, then allow a device. The code appears with its countdown.
Type "2"
Sleep 1.5s
Type "e"
Wait+Screen@10s /Pairing code/
Sleep 3.5s

# Travel laptop: find Studio Mac nearby and type the code it shows.
Ctrl+B
Type "o"
Sleep 1s
Type "2"
Sleep 1.5s
Type "f"
Wait+Screen@10s /Nearby/
Sleep 1.5s
Enter
Wait+Screen@10s /Code shown on/
Sleep 1.2s
Ctrl+T
Sleep 3.5s
Enter
Wait+Screen@15s /(?s)Paired\. Both computers saved.*Paired\. Both computers saved/
Sleep 6s

Hide
Ctrl+B
Type "d"
Sleep 1s
Type "demo_stop" Enter
Sleep 2s
