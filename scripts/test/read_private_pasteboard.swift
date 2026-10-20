// Native verification helper. It cannot open the user's general pasteboard.
import AppKit
let name = CommandLine.arguments.count == 2 ? CommandLine.arguments[1] : ""
guard name.hasPrefix("org.clipsync.test."), name.count > "org.clipsync.test.".count else {
    fputs("A private ClipSync test pasteboard name is required\n", stderr)
    exit(2)
}
let board = NSPasteboard(name: NSPasteboard.Name(name))
print(board.string(forType: .string) ?? "")
