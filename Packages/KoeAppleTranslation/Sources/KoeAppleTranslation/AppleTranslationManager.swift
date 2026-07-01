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
            // The language pair is supported by the framework, but the source language
            // model is not installed locally yet. Apple requires downloading it first.
            throw NSError(domain: "nz.owo.koe.apple-translation", code: 4, userInfo: [
                NSLocalizedDescriptionKey: "Source language is not installed for Apple Translation. Please install it in System Settings > Apple Intelligence & Siri > Language.",
            ])
        case .installed:
            break
        }

        let session = TranslationSession(installedSource: sourceLanguage, target: targetLanguage)
        let response = try await session.translate(sourceText)
        return response.targetText
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
