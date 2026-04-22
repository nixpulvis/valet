import Foundation
import Valet

/// Password-free view of a record, mirroring Rust's `RecordIndex` entry.
/// This is what `list` returns; use `fetch` to materialize the password.
public struct RecordIndexEntry: Sendable, Identifiable, Hashable {
    public let id: String
    public let label: String
    public let username: String?
    public let extras: [String: String]
}

/// Full record with a materialized password, mirroring Rust's `Record`.
/// Returned from `fetch` when the password is actually needed.
public struct Record: Sendable, Identifiable, Hashable {
    public let id: String
    public let label: String
    public let username: String?
    public let extras: [String: String]
    public let password: String
}

public struct ValetError: Error, CustomStringConvertible {
    public let code: Int32
    public let message: String
    public var description: String { "ffi error \(code): \(message)" }
}

public final class ValetClient: @unchecked Sendable {
    private let handle: OpaquePointer

    private init(handle: OpaquePointer) {
        self.handle = handle
    }

    deinit {
        valet_ffi_client_free(handle)
    }

    /// App Group identifier shared by the host app and the AutoFill
    /// extension. The shared container is the only filesystem region that
    /// the sandboxed extension and the (sandboxed or not) daemon can both
    /// reach under the same path, so the Unix socket has to live inside it.
    public static let appGroup = "group.com.nixpulvis.valet"

    /// On-disk path the Swift side hands to `valet_ffi_client_new_socket`.
    /// Also the path the daemon must be started with (`VALET_SOCKET=...`)
    /// so both endpoints agree. Throws when the group container has not
    /// been provisioned (first launch before any entitled process ran), so
    /// the caller can surface a real error instead of crashing.
    public static func socketPath() throws -> String {
        guard let url = FileManager.default.containerURL(
            forSecurityApplicationGroupIdentifier: appGroup
        ) else {
            throw ValetError(
                code: -1,
                message: "App Group container \(appGroup) is not provisioned; check entitlements"
            )
        }
        return url.appendingPathComponent("valet.sock").path
    }

    public static func `default`() throws -> ValetClient {
        var ptr: OpaquePointer?
        #if VALET_FFI_PROTOCOL_EMBEDDED
        try dbPath().withCString { try check(valet_ffi_client_new_embedded($0, &ptr)) }
        #else
        try socketPath().withCString { try check(valet_ffi_client_new_socket($0, &ptr)) }
        #endif
        return ValetClient(handle: ptr!)
    }

    /// SQLite database path for `valet_ffi_client_new_embedded`. Lives
    /// inside the same App Group container as the socket so the host
    /// app and the sandboxed extension share one DB.
    public static func dbPath() throws -> String {
        guard let url = FileManager.default.containerURL(
            forSecurityApplicationGroupIdentifier: appGroup
        ) else {
            throw ValetError(
                code: -1,
                message: "App Group container \(appGroup) is not provisioned; check entitlements"
            )
        }
        return url.appendingPathComponent("valet.sqlite").path
    }

    /// Currently-unlocked usernames (daemon cache). Empty if the daemon is
    /// fully locked.
    public func status() async throws -> [String] {
        try await Task.detached(priority: .userInitiated) { [handle] in
            var list = ValetStrList(items: nil, count: 0)
            try check(valet_ffi_client_status(handle, &list))
            defer { valet_ffi_str_list_free(list) }
            return unpackStrings(list)
        }.value
    }

    /// Every registered username (DB query).
    public func listUsers() async throws -> [String] {
        try await Task.detached(priority: .userInitiated) { [handle] in
            var list = ValetStrList(items: nil, count: 0)
            try check(valet_ffi_client_list_users(handle, &list))
            defer { valet_ffi_str_list_free(list) }
            return unpackStrings(list)
        }.value
    }

    /// Unlock `username` by deriving the key from `password` server-side.
    /// The daemon caches the user until its idle timeout; subsequent calls
    /// for the same username succeed without re-unlocking.
    public func unlock(username: String, password: String) async throws {
        try await Task.detached(priority: .userInitiated) { [handle] in
            try username.withCString { userPtr in
                try password.withCString { pwPtr in
                    let pwLen = UInt(password.utf8.count)
                    try check(valet_ffi_client_unlock(handle, userPtr, pwPtr, pwLen))
                }
            }
        }.value
    }

    /// List records for `username` matching any of the given Valet queries.
    /// An empty array returns every record in every lot the user has access
    /// to. See `valet::record::Query` for the query grammar; a broad
    /// cross-lot regex looks like `~::~github\.com`.
    public func list(username: String, queries: [String]) async throws -> [RecordIndexEntry] {
        try await Task.detached(priority: .userInitiated) { [handle] in
            var index = ValetRecordIndex(entries: nil, count: 0)
            try username.withCString { userPtr in
                try queries.withCStrings { pointers, lengths in
                    try check(valet_ffi_client_list(
                        handle, userPtr, pointers, lengths, UInt(queries.count), &index
                    ))
                }
            }
            defer { valet_ffi_record_index_free(index) }
            return unpack(index)
        }.value
    }

    /// Domain-suffix search within a lot. Mirrors the browser extension's
    /// `find_records` call so both platforms use the same match semantics
    /// (symmetric suffix: record `github.com` matches query
    /// `gist.github.com` and vice-versa).
    ///
    /// TODO: swap for a `Query::Domain` variant on the cross-lot `list`
    /// RPC once it lands; see the TODO on `Request::FindRecords`.
    public func findRecords(
        username: String,
        lot: String,
        domain: String
    ) async throws -> [RecordIndexEntry] {
        try await Task.detached(priority: .userInitiated) { [handle] in
            var index = ValetRecordIndex(entries: nil, count: 0)
            try username.withCString { userPtr in
                try lot.withCString { lotPtr in
                    try domain.withCString { domainPtr in
                        try check(valet_ffi_client_find_records(
                            handle, userPtr, lotPtr, domainPtr,
                            UInt(domain.utf8.count), &index
                        ))
                    }
                }
            }
            defer { valet_ffi_record_index_free(index) }
            return unpack(index)
        }.value
    }

    public func fetch(username: String, uuid: String) async throws -> Record {
        try await Task.detached(priority: .userInitiated) { [handle] in
            try username.withCString { userPtr in
                try uuid.withCString { uuidPtr in
                    var record = ValetRecord(
                        uuid: ValetStr(ptr: nil, len: 0),
                        label: ValetStr(ptr: nil, len: 0),
                        username: ValetStr(ptr: nil, len: 0),
                        extras: nil,
                        extras_count: 0,
                        password: ValetStr(ptr: nil, len: 0)
                    )
                    try check(valet_ffi_client_fetch(
                        handle, userPtr, uuidPtr, UInt(uuid.utf8.count), &record
                    ))
                    defer { valet_ffi_record_free(record) }
                    return Record(
                        id: decode(record.uuid),
                        label: decode(record.label),
                        username: decodeOptional(record.username),
                        extras: decodeExtras(record.extras, count: record.extras_count),
                        password: decode(record.password)
                    )
                }
            }
        }.value
    }
}

private func check(_ code: Int32) throws {
    guard code != VALET_FFI_OK else { return }
    let message = valet_ffi_last_error().map(String.init(cString:)) ?? ""
    throw ValetError(code: code, message: message)
}

private func unpack(_ index: ValetRecordIndex) -> [RecordIndexEntry] {
    guard index.count > 0, let base = index.entries else { return [] }
    return UnsafeBufferPointer(start: base, count: Int(index.count)).map { e in
        RecordIndexEntry(
            id: decode(e.uuid),
            label: decode(e.label),
            username: decodeOptional(e.username),
            extras: decodeExtras(e.extras, count: e.extras_count)
        )
    }
}

private func decodeOptional(_ s: ValetStr) -> String? {
    guard s.len > 0, s.ptr != nil else { return nil }
    return decode(s)
}

private func unpackStrings(_ list: ValetStrList) -> [String] {
    guard list.count > 0, let base = list.items else { return [] }
    return UnsafeBufferPointer(start: base, count: Int(list.count)).map { decode($0) }
}

private func decode(_ s: ValetStr) -> String {
    guard s.len > 0, let ptr = s.ptr else { return "" }
    let bytes = UnsafeBufferPointer(
        start: UnsafeRawPointer(ptr).assumingMemoryBound(to: UInt8.self),
        count: Int(s.len)
    )
    return String(decoding: bytes, as: UTF8.self)
}

private func decodeExtras(_ base: UnsafeMutablePointer<ValetKv>?, count: UInt) -> [String: String] {
    guard count > 0, let base = base else { return [:] }
    var out: [String: String] = [:]
    let buffer = UnsafeBufferPointer(start: base, count: Int(count))
    for kv in buffer {
        out[decode(kv.key)] = decode(kv.value)
    }
    return out
}

private extension Array where Element == String {
    /// Pass the array as parallel `(char**, size_t*)` buffers to `body`. Both
    /// pointers are nil when the array is empty.
    func withCStrings<R>(
        _ body: (UnsafePointer<UnsafePointer<CChar>?>?, UnsafePointer<UInt>?) throws -> R
    ) rethrows -> R {
        if isEmpty { return try body(nil, nil) }
        let utf8: [ContiguousArray<CChar>] = map { $0.utf8CString }
        let lengths: [UInt] = map { UInt($0.utf8.count) }
        return try utf8.map { $0.withUnsafeBufferPointer { $0.baseAddress } }
            .withUnsafeBufferPointer { ptrs in
                try lengths.withUnsafeBufferPointer { lens in
                    try body(ptrs.baseAddress, lens.baseAddress)
                }
            }
    }
}
