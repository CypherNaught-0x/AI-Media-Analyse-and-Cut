// Apple SpeechAnalyzer bridge for AI Media Cutter.
//
// SpeechAnalyzer/SpeechTranscriber (macOS 26+) are Swift-only async APIs, so
// the app drives them out of process through this helper rather than linking
// Swift into the Rust binary. That also keeps the app itself launchable on
// older macOS versions: only this helper needs macOS 26.
//
// Protocol (same as the CrisperWhisper runner): one JSON request object on
// stdin, newline-delimited JSON objects on stdout:
//
//     {"type": "progress", "message": "..."}
//     {"type": "result", ...}
//     {"type": "error", "message": "...", "kind": "..."}
//
// Actions:
//   probe       -> availability and supported/installed locales
//   install     -> download the on-device model for `locale`
//   transcribe  -> words with start/end/confidence for `audioPath` in `locale`

import AVFoundation
import Foundation
import Speech

struct Request: Decodable {
    var action: String?
    var locale: String?
    var audioPath: String?
}

func emit(_ payload: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: payload, options: []) else {
        return
    }
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data([0x0A]))
}

func progress(_ message: String) {
    emit(["type": "progress", "message": message])
}

func fail(_ message: String, kind: String = "runtime") -> Never {
    emit(["type": "error", "message": message, "kind": kind])
    exit(1)
}

func identifiers(_ locales: [Locale]) -> [String] {
    locales.map { $0.identifier(.bcp47) }.sorted()
}

func clock(_ seconds: Double) -> String {
    let total = Int(seconds.rounded())
    let (hours, rest) = (total / 3600, total % 3600)
    let (minutes, secs) = (rest / 60, rest % 60)
    return hours > 0
        ? String(format: "%d:%02d:%02d", hours, minutes, secs)
        : String(format: "%d:%02d", minutes, secs)
}

func makeTranscriber(_ locale: Locale) -> SpeechTranscriber {
    SpeechTranscriber(
        locale: locale,
        transcriptionOptions: [],
        reportingOptions: [],
        attributeOptions: [.audioTimeRange, .transcriptionConfidence]
    )
}

func resolveLocale(_ identifier: String?) async -> Locale {
    guard let identifier, !identifier.isEmpty else {
        fail("No locale was given.", kind: "input")
    }
    guard let locale = await SpeechTranscriber.supportedLocale(
        equivalentTo: Locale(identifier: identifier)
    ) else {
        fail("Apple Speech does not support the locale '\(identifier)'.", kind: "unsupported_locale")
    }
    return locale
}

/// Download the on-device model for `locale` if it is not installed yet.
func ensureInstalled(_ transcriber: SpeechTranscriber, _ locale: Locale) async throws {
    let installed = await SpeechTranscriber.installedLocales
    if installed.contains(where: { $0.identifier(.bcp47) == locale.identifier(.bcp47) }) {
        return
    }
    guard let request = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) else {
        return
    }

    let name = locale.identifier(.bcp47)
    progress("Downloading the Apple Speech model for \(name)...")
    let reporter = Task {
        var last = -1
        while !Task.isCancelled {
            let percent = Int(request.progress.fractionCompleted * 100)
            if percent != last {
                progress("Downloading the Apple Speech model for \(name): \(percent)%")
                last = percent
            }
            try? await Task.sleep(nanoseconds: 500_000_000)
        }
    }
    defer { reporter.cancel() }
    try await request.downloadAndInstall()
    progress("Apple Speech model for \(name) installed.")
}

func probe() async {
    emit([
        "type": "result",
        "available": SpeechTranscriber.isAvailable,
        "supportedLocales": identifiers(await SpeechTranscriber.supportedLocales),
        "installedLocales": identifiers(await SpeechTranscriber.installedLocales),
        "osVersion": ProcessInfo.processInfo.operatingSystemVersionString,
    ])
}

func install(_ request: Request) async {
    let locale = await resolveLocale(request.locale)
    do {
        try await ensureInstalled(makeTranscriber(locale), locale)
    } catch {
        fail("Downloading the Apple Speech model failed: \(error.localizedDescription)", kind: "install")
    }
    emit([
        "type": "result",
        "locale": locale.identifier(.bcp47),
        "installedLocales": identifiers(await SpeechTranscriber.installedLocales),
    ])
}

func transcribe(_ request: Request) async {
    guard let path = request.audioPath, FileManager.default.fileExists(atPath: path) else {
        fail("Audio file not found: \(request.audioPath ?? "")", kind: "input")
    }
    let locale = await resolveLocale(request.locale)
    let transcriber = makeTranscriber(locale)

    do {
        try await ensureInstalled(transcriber, locale)
    } catch {
        fail("Downloading the Apple Speech model failed: \(error.localizedDescription)", kind: "install")
    }

    let file: AVAudioFile
    do {
        file = try AVAudioFile(forReading: URL(fileURLWithPath: path))
    } catch {
        fail("Cannot read the audio file: \(error.localizedDescription)", kind: "input")
    }
    let duration = Double(file.length) / file.processingFormat.sampleRate
    progress("Transcribing \(clock(duration)) of audio with Apple Speech (\(locale.identifier(.bcp47)))...")

    let started = Date()
    // Results arrive in order; collect them concurrently with the analysis.
    let collector = Task { () throws -> [[String: Any]] in
        var words: [[String: Any]] = []
        var lastReported = 0.0
        for try await result in transcriber.results {
            appendWords(of: result, to: &words)
            let reached = result.range.end.seconds
            if reached.isFinite, reached - lastReported >= 30 || reached >= duration {
                lastReported = reached
                progress("Transcribing with Apple Speech: \(clock(min(reached, duration))) of \(clock(duration))")
            }
        }
        return words
    }

    let words: [[String: Any]]
    do {
        let analyzer = try await SpeechAnalyzer(
            inputAudioFile: file, modules: [transcriber], finishAfterFile: true
        )
        words = try await collector.value
        // Keep the analyzer alive until every result has been delivered.
        withExtendedLifetime(analyzer) {}
    } catch {
        collector.cancel()
        fail("Apple Speech transcription failed: \(error.localizedDescription)", kind: "inference")
    }

    emit([
        "type": "result",
        "locale": locale.identifier(.bcp47),
        "duration": duration,
        "processingTime": Date().timeIntervalSince(started),
        "words": words,
    ])
}

/// Flatten one result's attributed text into timed words.
///
/// Each run carries its own audio time range (in practice one word per run).
/// A run without a time range (stray punctuation) is attached to the word
/// before it rather than dropped.
func appendWords(of result: SpeechTranscriber.Result, to words: inout [[String: Any]]) {
    for run in result.text.runs {
        let text = String(result.text[run.range].characters)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if text.isEmpty {
            continue
        }
        guard let range = run.audioTimeRange, range.start.seconds.isFinite, range.end.seconds.isFinite else {
            if var previous = words.popLast() {
                previous["text"] = (previous["text"] as? String ?? "") + text
                words.append(previous)
            }
            continue
        }
        var word: [String: Any] = [
            "text": text,
            "start": range.start.seconds,
            "end": range.end.seconds,
        ]
        if let confidence = run.transcriptionConfidence {
            word["confidence"] = confidence
        }
        words.append(word)
    }
}

@main
struct AppleSpeechHelper {
    static func main() async {
        let input = FileHandle.standardInput.readDataToEndOfFile()
        let request: Request
        do {
            request = input.isEmpty ? Request() : try JSONDecoder().decode(Request.self, from: input)
        } catch {
            fail("Invalid request JSON: \(error.localizedDescription)", kind: "protocol")
        }

        switch (request.action ?? "transcribe").lowercased() {
        case "probe":
            await probe()
        case "install":
            await install(request)
        case "transcribe":
            await transcribe(request)
        case let action:
            fail("Unknown action '\(action)'.", kind: "protocol")
        }
    }
}
