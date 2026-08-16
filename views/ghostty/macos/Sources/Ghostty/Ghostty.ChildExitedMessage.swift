import Foundation
import GhosttyKit
import SwiftUI

extension Ghostty {
    struct ChildExitedMessage {
        enum Level {
            case success, error
        }
        let text: String
        let level: Level

        init(_ message: ghostty_surface_message_childexited_s) {
            switch Int(message.exit_code) {
            case Int(EXIT_SUCCESS):
                level = .success
            default:
                level = .error
            }
            text = "Process exited. Press any key to close the terminal."
        }
    }
}
