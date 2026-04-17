import Foundation
import ValetIPC

/// Swift-idiomatic view of a single Valet record, derived from the C
/// `ValetRecordView` that the Rust FFI layer emits.
public struct RecordView: Sendable, Identifiable, Hashable {
    public let id: String
    public let label: String
    public let username: String
    public let password: String
    public let url: String
}

public final class ValetClient: @unchecked Sendable {
    public enum ClientError: Error, CustomStringConvertible {
        case nullArg
        case invalidUTF8
        case io(String)
        case passwordTooLong
        case remote(String)
        case unknown(Int32, String)

        public var description: String {
            switch self {
            case .nullArg: return "null argument"
            case .invalidUTF8: return "invalid UTF-8"
            case .io(let msg): return "io: \(msg)"
            case .passwordTooLong: return "password too long"
            case .remote(let msg): return "remote: \(msg)"
            case .unknown(let code, let msg): return "unknown (\(code)): \(msg)"
            }
        }
    }

    private let handle: OpaquePointer

    private init(handle: OpaquePointer) {
        self.handle = handle
    }

    #if VALET_IPC_STUB
    public static func stub() throws -> ValetClient {
        var ptr: OpaquePointer?
        let rc = valet_ipc_client_new_stub(&ptr)
        guard rc == VALET_IPC_OK, let p = ptr else {
            throw error(for: rc)
        }
        return ValetClient(handle: p)
    }
    #endif

    public static func connect(socketPath: String) throws -> ValetClient {
        var ptr: OpaquePointer?
        let rc = socketPath.withCString { cstr in
            valet_ipc_client_connect(cstr, &ptr)
        }
        guard rc == VALET_IPC_OK, let p = ptr else {
            throw error(for: rc)
        }
        return ValetClient(handle: p)
    }

    public func unlock(username: String, password: String) async throws {
        try await Task.detached(priority: .userInitiated) { [handle] in
            let rc = username.withCString { userCStr -> Int32 in
                password.withCString { passCStr in
                    valet_ipc_client_unlock(
                        handle,
                        userCStr,
                        passCStr,
                        UInt(password.utf8.count)
                    )
                }
            }
            guard rc == VALET_IPC_OK else {
                throw ValetClient.error(for: rc)
            }
        }.value
    }

    public func list(serviceIdentifiers: [String]) async throws -> [RecordView] {
        try await Task.detached(priority: .userInitiated) { [handle] in
            var list = ValetRecordList(items: nil, count: 0)
            let rc: Int32 = ValetClient.withCStringArray(serviceIdentifiers) {
                pointers,
                lengths in
                valet_ipc_client_list(
                    handle,
                    pointers,
                    lengths,
                    UInt(serviceIdentifiers.count),
                    &list
                )
            }
            guard rc == VALET_IPC_OK else {
                throw ValetClient.error(for: rc)
            }
            defer { valet_ipc_record_list_free(list) }
            return ValetClient.unpack(list)
        }.value
    }

    deinit {
        valet_ipc_client_free(handle)
    }

    private static func unpack(_ list: ValetRecordList) -> [RecordView] {
        guard list.count > 0, let base = list.items else { return [] }
        let buf = UnsafeBufferPointer(start: base, count: Int(list.count))
        return buf.map { view in
            RecordView(
                id: string(view.uuid),
                label: string(view.label),
                username: string(view.username),
                password: string(view.password),
                url: string(view.url)
            )
        }
    }

    private static func string(_ s: ValetStr) -> String {
        guard s.len > 0, let ptr = s.ptr else { return "" }
        let len = Int(s.len)
        let buf = UnsafeBufferPointer(
            start: ptr.withMemoryRebound(to: UInt8.self, capacity: len) { $0 },
            count: len
        )
        return String(decoding: buf, as: UTF8.self)
    }

    private static func error(for code: Int32) -> ClientError {
        let message: String = {
            guard let cstr = valet_ipc_last_error() else { return "" }
            return String(cString: cstr)
        }()
        switch code {
        case VALET_IPC_ERR_NULL_ARG: return .nullArg
        case VALET_IPC_ERR_INVALID_UTF8: return .invalidUTF8
        case VALET_IPC_ERR_IO: return .io(message)
        case VALET_IPC_ERR_PROTOCOL: return .remote(message)
        case VALET_IPC_ERR_PASSWORD_TOO_LONG: return .passwordTooLong
        default: return .unknown(code, message)
        }
    }

    /// Build a parallel `(char** pointers, size_t* lengths)` pair from a Swift
    /// array of strings, and pass both to the body. The pointers are only
    /// valid for the duration of the call.
    private static func withCStringArray<R>(
        _ strings: [String],
        _ body: (UnsafePointer<UnsafePointer<CChar>?>?, UnsafePointer<UInt>?) throws -> R
    ) rethrows -> R {
        if strings.isEmpty {
            return try body(nil, nil)
        }
        // Materialize each string's UTF-8 bytes into a stable buffer so the
        // pointer remains valid inside `body`.
        let utf8Buffers: [ContiguousArray<CChar>] = strings.map { s in
            var buf = ContiguousArray<CChar>()
            buf.reserveCapacity(s.utf8.count + 1)
            for byte in s.utf8 { buf.append(CChar(bitPattern: byte)) }
            buf.append(0)
            return buf
        }
        return try utf8Buffers.withUnsafeBufferPointer { bufs -> R in
            let pointers: [UnsafePointer<CChar>?] = (0..<bufs.count).map { i in
                bufs[i].withUnsafeBufferPointer { $0.baseAddress }
            }
            let lengths: [UInt] = strings.map { UInt($0.utf8.count) }
            return try pointers.withUnsafeBufferPointer { ptrBuf in
                try lengths.withUnsafeBufferPointer { lenBuf in
                    try body(ptrBuf.baseAddress, lenBuf.baseAddress)
                }
            }
        }
    }
}
