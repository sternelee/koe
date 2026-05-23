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
        let sourceLanguage = sourceLang.flatMap(Self.language)
            ?? Locale.preferredLanguages.compactMap(Self.language).first
        guard let sourceLanguage, let targetLanguage = Self.language(targetLang) else {
            throw NSError(domain: "nz.owo.koe.apple-translation", code: 1, userInfo: [
                NSLocalizedDescriptionKey: "Apple Translation requires a source language or a preferred system language, plus a target language.",
            ])
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
