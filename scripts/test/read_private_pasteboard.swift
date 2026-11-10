// Native verification helper. It cannot open the user's general pasteboard.
import AppKit
let name = CommandLine.arguments.count >= 2 ? CommandLine.arguments[1] : ""
guard name.hasPrefix("org.clipsync.test."), name.count > "org.clipsync.test.".count else {
    fputs("A private ClipSync test pasteboard name is required\n", stderr)
    exit(2)
}
let board = NSPasteboard(name: NSPasteboard.Name(name))
if CommandLine.arguments.count == 3 && CommandLine.arguments[2] == "--png-base64" {
    print(board.data(forType: .png)?.base64EncodedString() ?? "")
} else {
    print(board.string(forType: .string) ?? "")
}
