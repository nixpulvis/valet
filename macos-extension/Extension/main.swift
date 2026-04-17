import Foundation

@_silgen_name("NSExtensionMain")
func NSExtensionMain() -> Int32

@main
enum ExtensionEntry {
    static func main() {
        exit(NSExtensionMain())
    }
}
