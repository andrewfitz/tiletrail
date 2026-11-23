import Foundation
import TileTrailC

enum AnalysisFailure: Error {
    case input
    case abi
    case analyzer(Int32, String)
}

func check(_ status: Int32, _ error: inout tiletrail_error_t) throws {
    guard status == TILETRAIL_OK else {
        let message = withUnsafeBytes(of: &error.message) { bytes in
            String(decoding: bytes.prefix(while: { $0 != 0 }), as: UTF8.self)
        }
        throw AnalysisFailure.analyzer(status, message)
    }
}

func run(_ args: [String]) throws {
    guard tiletrail_abi_version() == TILETRAIL_ABI_VERSION else {
        throw AnalysisFailure.abi
    }
    guard args.count == 7,
          let width = UInt32(args[2]), width > 0,
          let height = UInt32(args[3]), height > 0,
          let start = Int64(args[4]),
          let numerator = UInt32(args[5]), let denominator = UInt32(args[6]) else {
        throw AnalysisFailure.input
    }

    let stride = Int(width) * 3
    let (frameLength, frameOverflow) = stride.multipliedReportingOverflow(by: Int(height))
    guard !frameOverflow else { throw AnalysisFailure.input }
    let bytes = try Data(contentsOf: URL(fileURLWithPath: args[1]))
    guard !bytes.isEmpty, bytes.count.isMultiple(of: frameLength) else {
        throw AnalysisFailure.input
    }
    var time = tiletrail_timestamp_t(pts: start, numerator: numerator, denominator: denominator)
    var options = tiletrail_config_t(mode: UInt32(TILETRAIL_MODE_CPU_ONLY),
                                     sampling: UInt32(TILETRAIL_SAMPLING_EVERY_FRAME))
    var analyzer: OpaquePointer?
    var error = tiletrail_error_t()
    try check(tiletrail_analyzer_create(&options, time, &analyzer, &error), &error)
    defer { tiletrail_analyzer_free(analyzer) }
    try bytes.withUnsafeBytes { input in
        for offset in Swift.stride(from: 0, to: bytes.count, by: frameLength) {
            guard time.pts < Int64.max else { throw AnalysisFailure.input }
            let pixels = input.baseAddress!.assumingMemoryBound(to: UInt8.self)
            var frame = tiletrail_frame_t(data: pixels.advanced(by: offset),
                                          len: UInt64(frameLength), stride: UInt64(stride),
                                          width: width, height: height,
                                          format: UInt32(TILETRAIL_FORMAT_RGB24), rotation: 0,
                                          pixel_aspect_num: 1, pixel_aspect_den: 1, timestamp: time)
            try check(tiletrail_analyzer_push(analyzer, &frame, &error), &error)
            time.pts += 1
        }
    }

    var output = tiletrail_buffer_t()
    defer { tiletrail_buffer_free(&output) }
    try check(tiletrail_analyzer_result_json(analyzer, time, &output, &error), &error)
    guard let data = output.data else { throw AnalysisFailure.abi }
    FileHandle.standardOutput.write(Data(bytes: data, count: Int(output.len)))
}

do {
    try run(CommandLine.arguments)
} catch {
    FileHandle.standardError.write(Data("\(error)\n".utf8))
    exit(EXIT_FAILURE)
}
