import Foundation
import Translation

final class AppleTranslationManager {
    static let shared = AppleTranslationManager()

    private init() {}

    func translateBlocking(sourceText: String, sourceLang: String?, targetLang: String) -> String {
        guard #available(macOS 26.0, *) else {
            return "[error] Apple Translation requires a newer macOS version"
        }

        let semaphore = DispatchSemaphore(value: 0)
        var output = ""

        Task {
            do {
                output = try await self.translate(
                    sourceText: sourceText,
                    sourceLang: sourceLang,
                    targetLang: targetLang
                )
            } catch let translationError as TranslationError {
                output = "[error] Apple Translation: \(translationError.localizedDescription)"
            } catch {
                output = "[error] \(error.localizedDescription)"
            }
            semaphore.signal()
        }

        semaphore.wait()
        return output
    }

    @available(macOS 26.0, *)
    private func translate(sourceText: String, sourceLang: String?, targetLang: String) async throws -> String {
        guard let targetLanguage = Self.language(targetLang) else {
            throw NSError(domain: "nz.owo.koe.apple-translation", code: 2, userInfo: [
                NSLocalizedDescriptionKey: "Apple Translation requires a valid target language code (e.g. en, zh-Hans, ja).",
            ])
        }

        // Resolve source language. Prefer the caller-supplied value, then the system's
        // preferred languages. `installedSource:` requires an *installed* source language,
        // so we check availability first and report a clear error instead of letting the
        // session fail with the generic "Unable to Translate".
        let sourceLanguage: Locale.Language? = sourceLang.flatMap(Self.language)
            ?? Locale.preferredLanguages.compactMap(Self.language).first

        guard let sourceLanguage else {
            throw NSError(domain: "nz.owo.koe.apple-translation", code: 5, userInfo: [
                NSLocalizedDescriptionKey: "Apple Translation could not determine the source language.",
            ])
        }

        let availability = LanguageAvailability()
        let status = await availability.status(from: sourceLanguage, to: targetLanguage)
        switch status {
        case .unsupported:
            throw NSError(domain: "nz.owo.koe.apple-translation", code: 3, userInfo: [
                NSLocalizedDescriptionKey: "Apple Translation does not support this source/target language pair.",
            ])
        case .supported:
            // The language pair is supported by the framework, but the on-device
            // source language model is not installed yet. Apple downloads
            // supported language models in the background (typically triggered
            // by the availability query), so instead of failing immediately we
            // request the download and wait for it to become ready. Without this
            // the very first translation attempt always errors out and only the
            // next attempt succeeds once the background download finishes.
            try await ensureLanguageInstalled(
                availability: availability,
                source: sourceLanguage,
                target: targetLanguage
            )
        case .installed:
            break
        @unknown default:
            break
        }

        let session = TranslationSession(installedSource: sourceLanguage, target: targetLanguage)
        let response = try await session.translate(sourceText)
        return response.targetText
    }

    /// Best-effort download + bounded wait for an Apple Translation language pair
    /// that is `.supported` but not yet `.installed`.
    ///
    /// `prepareTranslation()` is the documented way to ask the system to download
    /// the language models. It is only effective when the session can request
    /// downloads (`canRequestDownloads`); sessions created directly via
    /// `init(installedSource:target:)` outside of a SwiftUI `translationTask` may
    /// not be able to, so we additionally poll `LanguageAvailability.status` for
    /// the background download the system performs on its own.
    @available(macOS 26.0, *)
    private func ensureLanguageInstalled(
        availability: LanguageAvailability,
        source: Locale.Language,
        target: Locale.Language
    ) async throws {
        let session = TranslationSession(installedSource: source, target: target)
        if session.canRequestDownloads {
            // Trigger the download prompt if the framework allows it from this
            // context. Errors here (e.g. no UI to show the permission prompt)
            // are non-fatal; the polling below still waits for the download.
            try? await session.prepareTranslation()
        }

        // Poll for the background install to finish. Language models can take a
        // few seconds to download; cap the wait so we never block indefinitely.
        let pollIntervalNanos: UInt64 = 500_000_000 // 0.5s
        let maxAttempts = 60 // up to ~30s
        for _ in 0..<maxAttempts {
            let status = await availability.status(from: source, to: target)
            if status == .installed {
                return
            }
            try? await Task.sleep(nanoseconds: pollIntervalNanos)
        }

        throw NSError(domain: "nz.owo.koe.apple-translation", code: 4, userInfo: [
            NSLocalizedDescriptionKey: "Source language is not installed for Apple Translation. Please install it in System Settings > Apple Intelligence & Siri > Language.",
        ])
    }

    @available(macOS 26.0, *)
    private static func language(_ code: String) -> Locale.Language? {
        let trimmed = code.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty || trimmed.caseInsensitiveCompare("auto") == .orderedSame {
            return nil
        }
        return Locale.Language(identifier: trimmed)
    }
}
