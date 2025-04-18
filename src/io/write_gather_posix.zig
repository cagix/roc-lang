//! POSIX-specific implementation of gather I/O using writev.

const std = @import("std");
const os = std.os;
const posix = std.posix;
const Allocator = std.mem.Allocator;

const write_gather = @import("write_gather.zig");
const WriteGatherError = write_gather.WriteGatherError;

/// POSIX-specific implementation of AlignedBuffer (simpler, no special alignment needed)
pub const PosixAlignedBuffer = struct {
    /// The buffer slice
    buffer: []u8,

    /// Initializes a new PosixAlignedBuffer
    pub fn init(allocator: Allocator, size: usize, _: std.fs.File) Allocator.Error!PosixAlignedBuffer {
        // On POSIX, no special alignment is necessary
        const buffer = try allocator.alloc(u8, size);
        return PosixAlignedBuffer{
            .buffer = buffer,
        };
    }

    /// Frees the buffer
    pub fn deinit(self: PosixAlignedBuffer, allocator: Allocator) void {
        allocator.free(self.buffer);
    }
};

/// Implementation of writeGather for POSIX using pwritev
pub fn writeGather(
    file_handle: std.fs.File,
    buffers: []*const PosixAlignedBuffer,
    offset: u64,
) WriteGatherError!usize {
    // For POSIX, we need to convert to iovec
    const iovecs = std.heap.page_allocator.alloc(posix.iovec_const, buffers.len) catch {
        return WriteGatherError.OutOfMemory;
    };
    defer std.heap.page_allocator.free(iovecs);

    // Fill the iovec structs directly from AlignedBuffer
    for (buffers, 0..) |buffer, i| {
        iovecs[i] = .{
            .base = buffer.buffer.ptr,
            .len = buffer.buffer.len,
        };
    }

    // Directly use posix.pwritev without any platform-specific checks
    const result = posix.pwritev(file_handle.handle, iovecs, offset) catch |err|
        return translateError(err);

    return @intCast(result);
}

/// Translates POSIX-specific errors to WriteGatherError
fn translateError(err: anyerror) WriteGatherError {
    return switch (err) {
        error.AccessDenied => WriteGatherError.AccessDenied,
        error.BadPathName => WriteGatherError.BadPathName,
        error.ConnectionResetByPeer => WriteGatherError.ConnectionResetByPeer,
        error.ConnectionTimedOut => WriteGatherError.ConnectionTimedOut,
        error.DeviceBusy => WriteGatherError.DeviceBusy,
        error.FileNotFound => WriteGatherError.FileNotFound,
        error.InputOutput => WriteGatherError.InputOutput,
        error.InvalidArgument => WriteGatherError.InvalidArgument,
        error.IsDir => WriteGatherError.IsDirectory,
        error.NotOpenForWriting => WriteGatherError.NotOpenForWriting,
        error.OperationAborted => WriteGatherError.OperationAborted,
        error.OutOfMemory => WriteGatherError.OutOfMemory,
        error.PathAlreadyExists => WriteGatherError.PathAlreadyExists,
        error.ProcessFdQuotaExceeded => WriteGatherError.ProcessFdQuotaExceeded,
        error.SystemFdQuotaExceeded => WriteGatherError.SystemFdQuotaExceeded,
        error.SystemResources => WriteGatherError.SystemResources,
        error.WouldBlock => WriteGatherError.WouldBlock,
        else => WriteGatherError.Unexpected,
    };
}