import Foundation
import Valetd

/// Password-free view of a record, mirroring Rust's `RecordIndex` entry.
/// This is what `list` returns; use `fetch` to materialize the password.
public struct RecordIndexEntry: Sendable, Identifiable, Hashable {
    public let id: String
    public let label: String
    public let extras: [String: String]
}

/// Full record with a materialized password, mirroring Rust's `Record`.
/// Returned from `fetch` when the password is actually needed.
public struct Record: Sendable, Identifiable, Hashable {
    public let id: String
    public let label: String
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
        valetd_ffi_client_free(handle)
    }

    public static func `default`() throws -> ValetClient {
        var ptr: OpaquePointer?
        #if VALETD_FFI_STUB
        try check(valetd_ffi_client_new_stub(&ptr))
        #else
        let path = NSString(string: "~/.local/share/valet/valet.sock").expandingTildeInPath
        try path.withCString { try check(valetd_ffi_client_connect($0, &ptr)) }
        #endif
        return ValetClient(handle: ptr!)
    }

    /// List records matching any of the given Valet queries. An empty array
    /// returns every record in the active lot. See `valet::record::Query` for
    /// the query grammar; a broad cross-lot regex looks like
    /// `~::~github\.com`.
    public func list(queries: [String]) async throws -> [RecordIndexEntry] {
        try await Task.detached(priority: .userInitiated) { [handle] in
            var index = ValetRecordIndex(entries: nil, count: 0)
            try queries.withCStrings { pointers, lengths in
                try check(valetd_ffi_client_list(
                    handle, pointers, lengths, UInt(queries.count), &index
                ))
            }
            defer { valet_ffi_record_index_free(index) }
            return unpack(index)
        }.value
    }

    public func fetch(uuid: String) async throws -> Record {
        try await Task.detached(priority: .userInitiated) { [handle] in
            try uuid.withCString { ptr in
                var record = ValetRecord(
                    uuid: ValetStr(ptr: nil, len: 0),
                    label: ValetStr(ptr: nil, len: 0),
                    extras: nil,
                    extras_count: 0,
                    password: ValetStr(ptr: nil, len: 0)
                )
                try check(valetd_ffi_client_fetch(
                    handle, ptr, UInt(uuid.utf8.count), &record
                ))
                defer { valet_ffi_record_free(record) }
                return Record(
                    id: decode(record.uuid),
                    label: decode(record.label),
                    extras: decodeExtras(record.extras, count: record.extras_count),
                    password: decode(record.password)
                )
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
            extras: decodeExtras(e.extras, count: e.extras_count)
        )
    }
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
