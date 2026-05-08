import Foundation

@_cdecl("koe_apple_translation_is_available")
public func koeAppleTranslationIsAvailable() -> Int32 {
    if #available(macOS 26.0, *) {
        return 1
    }
    return 0
}

@_cdecl("koe_apple_translation_translate")
public func koeAppleTranslationTranslate(
    _ sourceText: UnsafePointer<CChar>?,
    _ sourceLang: UnsafePointer<CChar>?,
    _ targetLang: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    guard let sourceText else {
        return strdup("[error] source text is required")
    }
    guard let targetLang else {
        return strdup("[error] target language is required")
    }

    let source = String(cString: sourceText)
    let sourceLanguage = sourceLang.map { String(cString: $0) }
    let targetLanguage = String(cString: targetLang)

    let translated = AppleTranslationManager.shared.translateBlocking(
        sourceText: source,
        sourceLang: sourceLanguage,
        targetLang: targetLanguage
    )
    return strdup(translated)
}

@_cdecl("koe_apple_translation_free_string")
public func koeAppleTranslationFreeString(_ ptr: UnsafeMutablePointer<CChar>?) {
    guard let ptr else { return }
    free(ptr)
}
