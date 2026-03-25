#import "SPSetupWizardWindowController.h"
#import <Cocoa/Cocoa.h>

static NSString *const kConfigDir = @".koe";
static NSString *const kConfigFile = @"config.yaml";
static NSString *const kDictionaryFile = @"dictionary.txt";
static NSString *const kSystemPromptFile = @"system_prompt.txt";

// Toolbar item identifiers
static NSToolbarItemIdentifier const kToolbarASR = @"asr";
static NSToolbarItemIdentifier const kToolbarLLM = @"llm";
static NSToolbarItemIdentifier const kToolbarHotkey = @"hotkey";
static NSToolbarItemIdentifier const kToolbarDictionary = @"dictionary";
static NSToolbarItemIdentifier const kToolbarSystemPrompt = @"system_prompt";

// ─── YAML helpers (minimal, line-based) ─────────────────────────────
// We parse/write the config.yaml with simple line-based logic to avoid
// pulling in a YAML library.  The config file is flat enough for this.

static NSString *configDirPath(void) {
    return [NSHomeDirectory() stringByAppendingPathComponent:kConfigDir];
}

static NSString *configFilePath(void) {
    return [configDirPath() stringByAppendingPathComponent:kConfigFile];
}

/// Read a top-level or nested YAML value.  `keyPath` e.g. @"asr.app_key".
/// Handles `key: "value"` and `key: value`.  Returns @"" if not found.
static NSString *yamlRead(NSString *yaml, NSString *keyPath) {
    NSArray<NSString *> *parts = [keyPath componentsSeparatedByString:@"."];
    if (parts.count == 0) return @"";

    NSArray<NSString *> *lines = [yaml componentsSeparatedByString:@"\n"];
    BOOL inSection = (parts.count == 1);  // top-level key needs no section
    NSString *section = parts.count > 1 ? parts[0] : nil;
    NSString *key = parts.lastObject;

    for (NSString *line in lines) {
        NSString *trimmed = [line stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceCharacterSet]];
        if (trimmed.length == 0 || [trimmed hasPrefix:@"#"]) continue;

        // Check section header (no leading whitespace, ends with :)
        if (section && !inSection) {
            if (![line hasPrefix:@" "] && ![line hasPrefix:@"\t"]) {
                NSString *sectionCandidate = [trimmed stringByReplacingOccurrencesOfString:@":" withString:@""];
                sectionCandidate = [sectionCandidate stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceCharacterSet]];
                if ([sectionCandidate isEqualToString:section]) {
                    inSection = YES;
                }
            }
            continue;
        }

        // If we were in a section and hit a new top-level key, stop
        if (section && inSection && ![line hasPrefix:@" "] && ![line hasPrefix:@"\t"]) {
            break;
        }

        // Match key
        NSString *prefix = [NSString stringWithFormat:@"%@:", key];
        if ([trimmed hasPrefix:prefix]) {
            NSString *value = [trimmed substringFromIndex:prefix.length];
            value = [value stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceCharacterSet]];
            // Strip inline comment first (before quote removal).
            if ([value hasPrefix:@"\""]) {
                NSRange closeQuote = [value rangeOfString:@"\"" options:0 range:NSMakeRange(1, value.length - 1)];
                if (closeQuote.location != NSNotFound) {
                    value = [value substringToIndex:closeQuote.location + 1];
                }
            } else {
                NSRange commentRange = [value rangeOfString:@" #"];
                if (commentRange.location != NSNotFound) {
                    value = [[value substringToIndex:commentRange.location]
                             stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceCharacterSet]];
                }
            }
            // Strip surrounding quotes
            if (value.length >= 2 &&
                [value hasPrefix:@"\""] && [value hasSuffix:@"\""]) {
                value = [value substringWithRange:NSMakeRange(1, value.length - 2)];
            }
            return value;
        }
    }
    return @"";
}

/// Set a value in the YAML string.  If the key exists, replace; otherwise append under section.
static NSString *yamlWrite(NSString *yaml, NSString *keyPath, NSString *value) {
    NSArray<NSString *> *parts = [keyPath componentsSeparatedByString:@"."];
    NSString *section = parts.count > 1 ? parts[0] : nil;
    NSString *key = parts.lastObject;

    // Quote the value if it contains special chars or is empty
    NSString *quotedValue;
    if (value.length == 0 ||
        [value rangeOfString:@" "].location != NSNotFound ||
        [value rangeOfString:@"#"].location != NSNotFound ||
        [value rangeOfString:@":"].location != NSNotFound ||
        [value rangeOfString:@"\""].location != NSNotFound ||
        [value rangeOfString:@"$"].location != NSNotFound ||
        [value rangeOfString:@"@"].location != NSNotFound ||
        [value hasPrefix:@"wss://"] || [value hasPrefix:@"https://"] || [value hasPrefix:@"http://"]) {
        quotedValue = [NSString stringWithFormat:@"\"%@\"", value];
    } else {
        quotedValue = value;
    }

    NSMutableArray<NSString *> *lines = [[yaml componentsSeparatedByString:@"\n"] mutableCopy];
    BOOL inSection = (section == nil);
    BOOL found = NO;
    NSString *indent = section ? @"  " : @"";

    for (NSInteger i = 0; i < (NSInteger)lines.count; i++) {
        NSString *line = lines[i];
        NSString *trimmed = [line stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceCharacterSet]];
        if (trimmed.length == 0 || [trimmed hasPrefix:@"#"]) continue;

        if (section && !inSection) {
            if (![line hasPrefix:@" "] && ![line hasPrefix:@"\t"]) {
                NSString *s = [trimmed stringByReplacingOccurrencesOfString:@":" withString:@""];
                s = [s stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceCharacterSet]];
                if ([s isEqualToString:section]) {
                    inSection = YES;
                }
            }
            continue;
        }

        if (section && inSection && ![line hasPrefix:@" "] && ![line hasPrefix:@"\t"]) {
            // Went past our section — insert before this line
            NSString *newLine = [NSString stringWithFormat:@"%@%@: %@", indent, key, quotedValue];
            [lines insertObject:newLine atIndex:i];
            found = YES;
            break;
        }

        NSString *prefix = [NSString stringWithFormat:@"%@:", key];
        if ([trimmed hasPrefix:prefix]) {
            NSString *newLine = [NSString stringWithFormat:@"%@%@: %@", indent, key, quotedValue];
            lines[i] = newLine;
            found = YES;
            break;
        }
    }

    if (!found) {
        if (section) {
            BOOL sectionFound = NO;
            NSInteger insertIdx = (NSInteger)lines.count;
            for (NSInteger i = 0; i < (NSInteger)lines.count; i++) {
                NSString *line = lines[i];
                NSString *trimmed = [line stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceCharacterSet]];
                if (!sectionFound) {
                    if (![line hasPrefix:@" "] && ![line hasPrefix:@"\t"] && trimmed.length > 0 && ![trimmed hasPrefix:@"#"]) {
                        NSString *s = [trimmed stringByReplacingOccurrencesOfString:@":" withString:@""];
                        s = [s stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceCharacterSet]];
                        if ([s isEqualToString:section]) {
                            sectionFound = YES;
                        }
                    }
                } else {
                    if (![line hasPrefix:@" "] && ![line hasPrefix:@"\t"] && trimmed.length > 0 && ![trimmed hasPrefix:@"#"]) {
                        insertIdx = i;
                        break;
                    }
                }
            }
            if (!sectionFound) {
                [lines addObject:@""];
                [lines addObject:[NSString stringWithFormat:@"%@:", section]];
                insertIdx = (NSInteger)lines.count;
            }
            NSString *newLine = [NSString stringWithFormat:@"%@%@: %@", indent, key, quotedValue];
            [lines insertObject:newLine atIndex:insertIdx];
        } else {
            [lines addObject:[NSString stringWithFormat:@"%@: %@", key, quotedValue]];
        }
    }

    return [lines componentsJoinedByString:@"\n"];
}

// ─── Window Controller ──────────────────────────────────────────────

@interface SPSetupWizardWindowController () <NSToolbarDelegate>

// Current pane
@property (nonatomic, copy) NSString *currentPaneIdentifier;
@property (nonatomic, strong) NSView *currentPaneView;

// ASR fields
@property (nonatomic, strong) NSTextField *asrAppKeyField;
@property (nonatomic, strong) NSTextField *asrAccessKeyField;
@property (nonatomic, strong) NSSecureTextField *asrAccessKeySecureField;
@property (nonatomic, strong) NSButton *asrAccessKeyToggle;

// LLM fields
@property (nonatomic, strong) NSButton *llmEnabledCheckbox;
@property (nonatomic, strong) NSTextField *llmBaseUrlField;
@property (nonatomic, strong) NSTextField *llmApiKeyField;
@property (nonatomic, strong) NSSecureTextField *llmApiKeySecureField;
@property (nonatomic, strong) NSButton *llmApiKeyToggle;
@property (nonatomic, strong) NSTextField *llmModelField;
@property (nonatomic, strong) NSButton *llmTestButton;
@property (nonatomic, strong) NSTextField *llmTestResultLabel;

// LLM max token parameter
@property (nonatomic, strong) NSPopUpButton *maxTokenParamPopup;

// Hotkey
@property (nonatomic, strong) NSPopUpButton *hotkeyPopup;

// Dictionary
@property (nonatomic, strong) NSTextView *dictionaryTextView;

// System Prompt
@property (nonatomic, strong) NSTextView *systemPromptTextView;

@end

@implementation SPSetupWizardWindowController

- (instancetype)init {
    NSWindow *window = [[NSWindow alloc]
        initWithContentRect:NSMakeRect(0, 0, 600, 400)
                  styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskMiniaturizable
                    backing:NSBackingStoreBuffered
                      defer:YES];
    window.title = @"Koe Settings";
    window.toolbarStyle = NSWindowToolbarStylePreference;

    self = [super initWithWindow:window];
    if (self) {
        [self setupToolbar];
        [self switchToPane:kToolbarASR];
        [self loadCurrentValues];
    }
    return self;
}

- (void)showWindow:(id)sender {
    [self loadCurrentValues];
    [self.window center];
    [self.window makeKeyAndOrderFront:sender];
    [NSApp activateIgnoringOtherApps:YES];
}

// ─── Toolbar ────────────────────────────────────────────────────────

- (void)setupToolbar {
    NSToolbar *toolbar = [[NSToolbar alloc] initWithIdentifier:@"KoeSettingsToolbar"];
    toolbar.delegate = self;
    toolbar.displayMode = NSToolbarDisplayModeIconAndLabel;
    toolbar.selectedItemIdentifier = kToolbarASR;
    self.window.toolbar = toolbar;
}

- (NSArray<NSToolbarItemIdentifier> *)toolbarAllowedItemIdentifiers:(NSToolbar *)toolbar {
    return @[kToolbarASR, kToolbarLLM, kToolbarHotkey, kToolbarDictionary, kToolbarSystemPrompt];
}

- (NSArray<NSToolbarItemIdentifier> *)toolbarDefaultItemIdentifiers:(NSToolbar *)toolbar {
    return @[kToolbarASR, kToolbarLLM, kToolbarHotkey, kToolbarDictionary, kToolbarSystemPrompt];
}

- (NSArray<NSToolbarItemIdentifier> *)toolbarSelectableItemIdentifiers:(NSToolbar *)toolbar {
    return @[kToolbarASR, kToolbarLLM, kToolbarHotkey, kToolbarDictionary, kToolbarSystemPrompt];
}

- (NSToolbarItem *)toolbar:(NSToolbar *)toolbar itemForItemIdentifier:(NSToolbarItemIdentifier)itemIdentifier willBeInsertedIntoToolbar:(BOOL)flag {
    NSToolbarItem *item = [[NSToolbarItem alloc] initWithItemIdentifier:itemIdentifier];
    item.target = self;
    item.action = @selector(toolbarItemClicked:);

    if ([itemIdentifier isEqualToString:kToolbarASR]) {
        item.label = @"ASR";
        item.image = [NSImage imageWithSystemSymbolName:@"mic.fill" accessibilityDescription:@"ASR"];
    } else if ([itemIdentifier isEqualToString:kToolbarLLM]) {
        item.label = @"LLM";
        item.image = [NSImage imageWithSystemSymbolName:@"cpu" accessibilityDescription:@"LLM"];
    } else if ([itemIdentifier isEqualToString:kToolbarHotkey]) {
        item.label = @"Hotkey";
        item.image = [NSImage imageWithSystemSymbolName:@"keyboard" accessibilityDescription:@"Hotkey"];
    } else if ([itemIdentifier isEqualToString:kToolbarDictionary]) {
        item.label = @"Dictionary";
        item.image = [NSImage imageWithSystemSymbolName:@"book" accessibilityDescription:@"Dictionary"];
    } else if ([itemIdentifier isEqualToString:kToolbarSystemPrompt]) {
        item.label = @"Prompt";
        item.image = [NSImage imageWithSystemSymbolName:@"text.bubble" accessibilityDescription:@"System Prompt"];
    }

    return item;
}

- (void)toolbarItemClicked:(NSToolbarItem *)sender {
    [self switchToPane:sender.itemIdentifier];
}

// ─── Pane Switching ─────────────────────────────────────────────────

- (void)switchToPane:(NSString *)identifier {
    if ([self.currentPaneIdentifier isEqualToString:identifier]) return;
    self.currentPaneIdentifier = identifier;

    // Remove old pane
    [self.currentPaneView removeFromSuperview];

    // Build new pane
    NSView *paneView;
    if ([identifier isEqualToString:kToolbarASR]) {
        paneView = [self buildAsrPane];
    } else if ([identifier isEqualToString:kToolbarLLM]) {
        paneView = [self buildLlmPane];
    } else if ([identifier isEqualToString:kToolbarHotkey]) {
        paneView = [self buildHotkeyPane];
    } else if ([identifier isEqualToString:kToolbarDictionary]) {
        paneView = [self buildDictionaryPane];
    } else if ([identifier isEqualToString:kToolbarSystemPrompt]) {
        paneView = [self buildSystemPromptPane];
    }

    if (!paneView) return;

    self.currentPaneView = paneView;
    self.window.toolbar.selectedItemIdentifier = identifier;

    // Resize window to fit pane with animation
    NSSize paneSize = paneView.frame.size;
    NSRect windowFrame = self.window.frame;
    CGFloat contentHeight = paneSize.height;
    CGFloat titleBarHeight = windowFrame.size.height - [self.window.contentView frame].size.height;
    CGFloat newHeight = contentHeight + titleBarHeight;
    CGFloat newWidth = paneSize.width;

    NSRect newFrame = NSMakeRect(
        windowFrame.origin.x + (windowFrame.size.width - newWidth) / 2.0,
        windowFrame.origin.y + windowFrame.size.height - newHeight,
        newWidth,
        newHeight
    );

    [self.window setFrame:newFrame display:YES animate:YES];

    // Add pane to window
    paneView.frame = [self.window.contentView bounds];
    paneView.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
    [self.window.contentView addSubview:paneView];

    // Reload values for this pane
    [self loadValuesForPane:identifier];
}

// ─── Build Panes ────────────────────────────────────────────────────

- (NSView *)buildAsrPane {
    CGFloat paneWidth = 600;
    CGFloat labelW = 130;
    CGFloat fieldX = labelW + 24;
    CGFloat fieldW = paneWidth - fieldX - 32;
    CGFloat rowH = 32;

    // Calculate content height
    CGFloat contentHeight = 220;
    NSView *pane = [[NSView alloc] initWithFrame:NSMakeRect(0, 0, paneWidth, contentHeight)];

    CGFloat y = contentHeight - 48;

    // Description
    NSTextField *desc = [self descriptionLabel:@"Configure the Doubao Streaming ASR service. You need credentials from the Volcengine console."];
    desc.frame = NSMakeRect(24, y - 10, paneWidth - 48, 36);
    [pane addSubview:desc];
    y -= 52;

    // App Key
    [pane addSubview:[self formLabel:@"App Key" frame:NSMakeRect(16, y, labelW, 22)]];
    self.asrAppKeyField = [self formTextField:NSMakeRect(fieldX, y, fieldW, 22) placeholder:@"Volcengine App ID"];
    [pane addSubview:self.asrAppKeyField];
    y -= rowH;

    // Access Key (secure by default)
    CGFloat eyeW = 28;
    CGFloat secFieldW = fieldW - eyeW - 4;
    [pane addSubview:[self formLabel:@"Access Key" frame:NSMakeRect(16, y, labelW, 22)]];
    self.asrAccessKeySecureField = [[NSSecureTextField alloc] initWithFrame:NSMakeRect(fieldX, y, secFieldW, 22)];
    self.asrAccessKeySecureField.placeholderString = @"Volcengine Access Token";
    self.asrAccessKeySecureField.font = [NSFont systemFontOfSize:13];
    [pane addSubview:self.asrAccessKeySecureField];
    self.asrAccessKeyField = [self formTextField:NSMakeRect(fieldX, y, secFieldW, 22) placeholder:@"Volcengine Access Token"];
    self.asrAccessKeyField.hidden = YES;
    [pane addSubview:self.asrAccessKeyField];
    self.asrAccessKeyToggle = [self eyeButtonWithFrame:NSMakeRect(fieldX + secFieldW + 4, y - 1, eyeW, 24)
                                                action:@selector(toggleAsrAccessKeyVisibility:)];
    [pane addSubview:self.asrAccessKeyToggle];
    y -= rowH + 16;

    // Save / Cancel buttons
    [self addButtonsToPane:pane atY:y width:paneWidth];

    return pane;
}

- (NSView *)buildLlmPane {
    CGFloat paneWidth = 600;
    CGFloat labelW = 130;
    CGFloat fieldX = labelW + 24;
    CGFloat fieldW = paneWidth - fieldX - 32;
    CGFloat rowH = 32;

    CGFloat contentHeight = 540;
    NSView *pane = [[NSView alloc] initWithFrame:NSMakeRect(0, 0, paneWidth, contentHeight)];

    CGFloat y = contentHeight - 48;

    // Description
    NSTextField *desc = [self descriptionLabel:@"Configure an OpenAI-compatible LLM for post-correction. When disabled, raw ASR output is used directly."];
    desc.frame = NSMakeRect(24, y - 10, paneWidth - 48, 36);
    [pane addSubview:desc];
    y -= 52;

    // Enabled toggle
    self.llmEnabledCheckbox = [NSButton checkboxWithTitle:@"Enable LLM Correction"
                                                   target:self
                                                   action:@selector(llmEnabledToggled:)];
    self.llmEnabledCheckbox.frame = NSMakeRect(fieldX, y, 300, 22);
    [pane addSubview:self.llmEnabledCheckbox];
    y -= rowH + 8;

    // Base URL
    [pane addSubview:[self formLabel:@"Base URL" frame:NSMakeRect(16, y, labelW, 22)]];
    self.llmBaseUrlField = [self formTextField:NSMakeRect(fieldX, y, fieldW, 22) placeholder:@"https://api.openai.com/v1"];
    [pane addSubview:self.llmBaseUrlField];
    y -= rowH;

    // API Key (secure by default)
    CGFloat eyeW = 28;
    CGFloat secFieldW = fieldW - eyeW - 4;
    [pane addSubview:[self formLabel:@"API Key" frame:NSMakeRect(16, y, labelW, 22)]];
    self.llmApiKeySecureField = [[NSSecureTextField alloc] initWithFrame:NSMakeRect(fieldX, y, secFieldW, 22)];
    self.llmApiKeySecureField.placeholderString = @"sk-...";
    self.llmApiKeySecureField.font = [NSFont systemFontOfSize:13];
    [pane addSubview:self.llmApiKeySecureField];
    self.llmApiKeyField = [self formTextField:NSMakeRect(fieldX, y, secFieldW, 22) placeholder:@"sk-..."];
    self.llmApiKeyField.hidden = YES;
    [pane addSubview:self.llmApiKeyField];
    self.llmApiKeyToggle = [self eyeButtonWithFrame:NSMakeRect(fieldX + secFieldW + 4, y - 1, eyeW, 24)
                                             action:@selector(toggleLlmApiKeyVisibility:)];
    [pane addSubview:self.llmApiKeyToggle];
    y -= rowH;

    // Model
    [pane addSubview:[self formLabel:@"Model" frame:NSMakeRect(16, y, labelW, 22)]];
    self.llmModelField = [self formTextField:NSMakeRect(fieldX, y, fieldW, 22) placeholder:@"gpt-5.4-nano"];
    [pane addSubview:self.llmModelField];
    y -= rowH + 4;

    // Max Token Parameter
    [pane addSubview:[self formLabel:@"Token Parameter" frame:NSMakeRect(16, y, labelW, 22)]];
    self.maxTokenParamPopup = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(fieldX, y - 2, 240, 26) pullsDown:NO];
    [self.maxTokenParamPopup addItemsWithTitles:@[
        @"max_completion_tokens",
        @"max_tokens",
    ]];
    [self.maxTokenParamPopup itemAtIndex:0].representedObject = @"max_completion_tokens";
    [self.maxTokenParamPopup itemAtIndex:1].representedObject = @"max_tokens";
    [pane addSubview:self.maxTokenParamPopup];
    y -= 36;

    // Hint text
    NSTextField *tokenHint = [self descriptionLabel:@"GPT-4o and older models use max_tokens. GPT-5 and reasoning models (o1/o3) use max_completion_tokens."];
    tokenHint.frame = NSMakeRect(fieldX, y, fieldW, 32);
    [pane addSubview:tokenHint];
    y -= 44;

    // Test button
    self.llmTestButton = [NSButton buttonWithTitle:@"Test Connection" target:self action:@selector(testLlmConnection:)];
    self.llmTestButton.bezelStyle = NSBezelStyleRounded;
    self.llmTestButton.frame = NSMakeRect(fieldX, y, 130, 28);
    [pane addSubview:self.llmTestButton];
    y -= 32;

    // Test result
    self.llmTestResultLabel = [NSTextField wrappingLabelWithString:@""];
    self.llmTestResultLabel.frame = NSMakeRect(fieldX, y - 36, fieldW, 42);
    self.llmTestResultLabel.font = [NSFont systemFontOfSize:12];
    self.llmTestResultLabel.selectable = YES;
    [pane addSubview:self.llmTestResultLabel];

    // Save / Cancel buttons
    [self addButtonsToPane:pane atY:16 width:paneWidth];

    return pane;
}

- (NSView *)buildHotkeyPane {
    CGFloat paneWidth = 600;
    CGFloat labelW = 130;
    CGFloat fieldX = labelW + 24;
    CGFloat rowH = 32;

    CGFloat contentHeight = 220;
    NSView *pane = [[NSView alloc] initWithFrame:NSMakeRect(0, 0, paneWidth, contentHeight)];

    CGFloat y = contentHeight - 48;

    // Description
    NSTextField *desc = [self descriptionLabel:@"Choose which key triggers voice input. Hold to record or double-press to toggle."];
    desc.frame = NSMakeRect(24, y - 10, paneWidth - 48, 36);
    [pane addSubview:desc];
    y -= 52;

    // Trigger Key
    [pane addSubview:[self formLabel:@"Trigger Key" frame:NSMakeRect(16, y, labelW, 22)]];

    self.hotkeyPopup = [[NSPopUpButton alloc] initWithFrame:NSMakeRect(fieldX, y - 2, 220, 26) pullsDown:NO];
    [self.hotkeyPopup addItemsWithTitles:@[
        @"Fn (Globe)",
        @"Left Option (\u2325)",
        @"Right Option (\u2325)",
        @"Left Command (\u2318)",
        @"Right Command (\u2318)",
    ]];
    [self.hotkeyPopup itemAtIndex:0].representedObject = @"fn";
    [self.hotkeyPopup itemAtIndex:1].representedObject = @"left_option";
    [self.hotkeyPopup itemAtIndex:2].representedObject = @"right_option";
    [self.hotkeyPopup itemAtIndex:3].representedObject = @"left_command";
    [self.hotkeyPopup itemAtIndex:4].representedObject = @"right_command";
    [pane addSubview:self.hotkeyPopup];
    y -= rowH + 16;

    // Save / Cancel buttons
    [self addButtonsToPane:pane atY:y width:paneWidth];

    return pane;
}

- (NSView *)buildDictionaryPane {
    CGFloat paneWidth = 600;
    CGFloat contentHeight = 440;
    NSView *pane = [[NSView alloc] initWithFrame:NSMakeRect(0, 0, paneWidth, contentHeight)];

    CGFloat y = contentHeight - 48;

    // Description
    NSTextField *desc = [self descriptionLabel:@"User dictionary \u2014 one term per line. These terms are prioritized during LLM correction. Lines starting with # are comments."];
    desc.frame = NSMakeRect(24, y - 10, paneWidth - 48, 36);
    [pane addSubview:desc];
    y -= 44;

    // Text editor
    CGFloat editorHeight = y - 56;
    NSScrollView *scrollView = [[NSScrollView alloc] initWithFrame:NSMakeRect(24, 56, paneWidth - 48, editorHeight)];
    scrollView.hasVerticalScroller = YES;
    scrollView.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
    scrollView.borderType = NSBezelBorder;

    self.dictionaryTextView = [[NSTextView alloc] initWithFrame:NSMakeRect(0, 0, paneWidth - 54, editorHeight)];
    self.dictionaryTextView.minSize = NSMakeSize(0, editorHeight);
    self.dictionaryTextView.maxSize = NSMakeSize(FLT_MAX, FLT_MAX);
    self.dictionaryTextView.verticallyResizable = YES;
    self.dictionaryTextView.horizontallyResizable = NO;
    self.dictionaryTextView.autoresizingMask = NSViewWidthSizable;
    self.dictionaryTextView.textContainer.containerSize = NSMakeSize(paneWidth - 54, FLT_MAX);
    self.dictionaryTextView.textContainer.widthTracksTextView = YES;
    self.dictionaryTextView.font = [NSFont monospacedSystemFontOfSize:12 weight:NSFontWeightRegular];
    self.dictionaryTextView.allowsUndo = YES;

    scrollView.documentView = self.dictionaryTextView;
    [pane addSubview:scrollView];

    // Save / Cancel buttons
    [self addButtonsToPane:pane atY:16 width:paneWidth];

    return pane;
}

- (NSView *)buildSystemPromptPane {
    CGFloat paneWidth = 600;
    CGFloat contentHeight = 440;
    NSView *pane = [[NSView alloc] initWithFrame:NSMakeRect(0, 0, paneWidth, contentHeight)];

    CGFloat y = contentHeight - 48;

    // Description
    NSTextField *desc = [self descriptionLabel:@"System prompt sent to the LLM for text correction. Edit to customize behavior."];
    desc.frame = NSMakeRect(24, y - 10, paneWidth - 48, 36);
    [pane addSubview:desc];
    y -= 44;

    // Text editor
    CGFloat editorHeight = y - 56;
    NSScrollView *scrollView = [[NSScrollView alloc] initWithFrame:NSMakeRect(24, 56, paneWidth - 48, editorHeight)];
    scrollView.hasVerticalScroller = YES;
    scrollView.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
    scrollView.borderType = NSBezelBorder;

    self.systemPromptTextView = [[NSTextView alloc] initWithFrame:NSMakeRect(0, 0, paneWidth - 54, editorHeight)];
    self.systemPromptTextView.minSize = NSMakeSize(0, editorHeight);
    self.systemPromptTextView.maxSize = NSMakeSize(FLT_MAX, FLT_MAX);
    self.systemPromptTextView.verticallyResizable = YES;
    self.systemPromptTextView.horizontallyResizable = NO;
    self.systemPromptTextView.autoresizingMask = NSViewWidthSizable;
    self.systemPromptTextView.textContainer.containerSize = NSMakeSize(paneWidth - 54, FLT_MAX);
    self.systemPromptTextView.textContainer.widthTracksTextView = YES;
    self.systemPromptTextView.font = [NSFont monospacedSystemFontOfSize:12 weight:NSFontWeightRegular];
    self.systemPromptTextView.allowsUndo = YES;

    scrollView.documentView = self.systemPromptTextView;
    [pane addSubview:scrollView];

    // Save / Cancel buttons
    [self addButtonsToPane:pane atY:16 width:paneWidth];

    return pane;
}

// ─── Shared button bar ──────────────────────────────────────────────

- (void)addButtonsToPane:(NSView *)pane atY:(CGFloat)y width:(CGFloat)paneWidth {
    NSButton *saveButton = [NSButton buttonWithTitle:@"Save" target:self action:@selector(saveConfig:)];
    saveButton.bezelStyle = NSBezelStyleRounded;
    saveButton.keyEquivalent = @"\r";
    saveButton.frame = NSMakeRect(paneWidth - 32 - 80, y, 80, 28);
    [pane addSubview:saveButton];

    NSButton *cancelButton = [NSButton buttonWithTitle:@"Cancel" target:self action:@selector(cancelSetup:)];
    cancelButton.bezelStyle = NSBezelStyleRounded;
    cancelButton.keyEquivalent = @"\033";
    cancelButton.frame = NSMakeRect(paneWidth - 32 - 80 - 88, y, 80, 28);
    [pane addSubview:cancelButton];
}

// ─── UI Helpers ─────────────────────────────────────────────────────

- (NSTextField *)formLabel:(NSString *)title frame:(NSRect)frame {
    NSTextField *label = [NSTextField labelWithString:title];
    label.frame = frame;
    label.alignment = NSTextAlignmentRight;
    label.font = [NSFont systemFontOfSize:13 weight:NSFontWeightMedium];
    label.textColor = [NSColor labelColor];
    return label;
}

- (NSTextField *)formTextField:(NSRect)frame placeholder:(NSString *)placeholder {
    NSTextField *field = [[NSTextField alloc] initWithFrame:frame];
    field.placeholderString = placeholder;
    field.font = [NSFont systemFontOfSize:13];
    field.lineBreakMode = NSLineBreakByTruncatingTail;
    field.usesSingleLineMode = YES;
    return field;
}

- (NSTextField *)descriptionLabel:(NSString *)text {
    NSTextField *label = [NSTextField wrappingLabelWithString:text];
    label.font = [NSFont systemFontOfSize:12];
    label.textColor = [NSColor secondaryLabelColor];
    return label;
}

- (NSButton *)eyeButtonWithFrame:(NSRect)frame action:(SEL)action {
    NSButton *button = [[NSButton alloc] initWithFrame:frame];
    button.bezelStyle = NSBezelStyleInline;
    button.bordered = NO;
    button.image = [NSImage imageWithSystemSymbolName:@"eye.slash" accessibilityDescription:@"Show"];
    button.imageScaling = NSImageScaleProportionallyUpOrDown;
    button.target = self;
    button.action = action;
    button.tag = 0; // 0 = hidden, 1 = visible
    return button;
}

- (void)toggleAsrAccessKeyVisibility:(NSButton *)sender {
    if (sender.tag == 0) {
        // Show plain text
        self.asrAccessKeyField.stringValue = self.asrAccessKeySecureField.stringValue;
        self.asrAccessKeySecureField.hidden = YES;
        self.asrAccessKeyField.hidden = NO;
        sender.image = [NSImage imageWithSystemSymbolName:@"eye" accessibilityDescription:@"Hide"];
        sender.tag = 1;
    } else {
        // Show secure
        self.asrAccessKeySecureField.stringValue = self.asrAccessKeyField.stringValue;
        self.asrAccessKeyField.hidden = YES;
        self.asrAccessKeySecureField.hidden = NO;
        sender.image = [NSImage imageWithSystemSymbolName:@"eye.slash" accessibilityDescription:@"Show"];
        sender.tag = 0;
    }
}

- (void)toggleLlmApiKeyVisibility:(NSButton *)sender {
    if (sender.tag == 0) {
        self.llmApiKeyField.stringValue = self.llmApiKeySecureField.stringValue;
        self.llmApiKeySecureField.hidden = YES;
        self.llmApiKeyField.hidden = NO;
        sender.image = [NSImage imageWithSystemSymbolName:@"eye" accessibilityDescription:@"Hide"];
        sender.tag = 1;
    } else {
        self.llmApiKeySecureField.stringValue = self.llmApiKeyField.stringValue;
        self.llmApiKeyField.hidden = YES;
        self.llmApiKeySecureField.hidden = NO;
        sender.image = [NSImage imageWithSystemSymbolName:@"eye.slash" accessibilityDescription:@"Show"];
        sender.tag = 0;
    }
}

// ─── Load / Save ────────────────────────────────────────────────────

- (void)loadCurrentValues {
    [self loadValuesForPane:self.currentPaneIdentifier];
}

- (void)loadValuesForPane:(NSString *)identifier {
    NSString *dir = configDirPath();
    NSString *configPath = configFilePath();
    NSString *yaml = [NSString stringWithContentsOfFile:configPath encoding:NSUTF8StringEncoding error:nil] ?: @"";

    if ([identifier isEqualToString:kToolbarASR]) {
        self.asrAppKeyField.stringValue = yamlRead(yaml, @"asr.app_key");
        NSString *accessKey = yamlRead(yaml, @"asr.access_key");
        self.asrAccessKeySecureField.stringValue = accessKey;
        self.asrAccessKeyField.stringValue = accessKey;
        // Reset to hidden state
        self.asrAccessKeySecureField.hidden = NO;
        self.asrAccessKeyField.hidden = YES;
        self.asrAccessKeyToggle.image = [NSImage imageWithSystemSymbolName:@"eye.slash" accessibilityDescription:@"Show"];
        self.asrAccessKeyToggle.tag = 0;
    } else if ([identifier isEqualToString:kToolbarLLM]) {
        NSString *enabled = yamlRead(yaml, @"llm.enabled");
        self.llmEnabledCheckbox.state = ([enabled isEqualToString:@"false"]) ? NSControlStateValueOff : NSControlStateValueOn;
        NSString *baseUrl = yamlRead(yaml, @"llm.base_url");
        self.llmBaseUrlField.stringValue = baseUrl.length > 0 ? baseUrl : @"https://api.openai.com/v1";
        NSString *apiKey = yamlRead(yaml, @"llm.api_key");
        self.llmApiKeySecureField.stringValue = apiKey;
        self.llmApiKeyField.stringValue = apiKey;
        self.llmApiKeySecureField.hidden = NO;
        self.llmApiKeyField.hidden = YES;
        self.llmApiKeyToggle.image = [NSImage imageWithSystemSymbolName:@"eye.slash" accessibilityDescription:@"Show"];
        self.llmApiKeyToggle.tag = 0;
        NSString *model = yamlRead(yaml, @"llm.model");
        self.llmModelField.stringValue = model.length > 0 ? model : @"gpt-5.4-nano";
        // Max token parameter
        NSString *maxTokenParam = yamlRead(yaml, @"llm.max_token_parameter");
        if (maxTokenParam.length == 0) maxTokenParam = @"max_completion_tokens";
        for (NSInteger i = 0; i < self.maxTokenParamPopup.numberOfItems; i++) {
            if ([[self.maxTokenParamPopup itemAtIndex:i].representedObject isEqualToString:maxTokenParam]) {
                [self.maxTokenParamPopup selectItemAtIndex:i];
                break;
            }
        }
        self.llmTestResultLabel.stringValue = @"";
        [self updateLlmFieldsEnabled];
    } else if ([identifier isEqualToString:kToolbarHotkey]) {
        NSString *triggerKey = yamlRead(yaml, @"hotkey.trigger_key");
        if (triggerKey.length == 0) triggerKey = @"fn";
        for (NSInteger i = 0; i < self.hotkeyPopup.numberOfItems; i++) {
            if ([[self.hotkeyPopup itemAtIndex:i].representedObject isEqualToString:triggerKey]) {
                [self.hotkeyPopup selectItemAtIndex:i];
                break;
            }
        }
    } else if ([identifier isEqualToString:kToolbarDictionary]) {
        NSString *dictPath = [dir stringByAppendingPathComponent:kDictionaryFile];
        NSString *dictContent = [NSString stringWithContentsOfFile:dictPath encoding:NSUTF8StringEncoding error:nil] ?: @"";
        [self.dictionaryTextView setString:dictContent];
    } else if ([identifier isEqualToString:kToolbarSystemPrompt]) {
        NSString *promptPath = [dir stringByAppendingPathComponent:kSystemPromptFile];
        NSString *promptContent = [NSString stringWithContentsOfFile:promptPath encoding:NSUTF8StringEncoding error:nil] ?: @"";
        [self.systemPromptTextView setString:promptContent];
    }
}

- (void)saveConfig:(id)sender {
    NSString *dir = configDirPath();

    // Ensure directory exists
    [[NSFileManager defaultManager] createDirectoryAtPath:dir
                              withIntermediateDirectories:YES
                                               attributes:nil
                                                    error:nil];

    // Read existing config.yaml (preserve structure)
    NSString *configPath = configFilePath();
    NSString *yaml = [NSString stringWithContentsOfFile:configPath encoding:NSUTF8StringEncoding error:nil] ?: @"";

    // Update ASR fields (always save — fields may be nil if pane not visited, check first)
    if (self.asrAppKeyField) {
        yaml = yamlWrite(yaml, @"asr.app_key", self.asrAppKeyField.stringValue);
        NSString *accessKey = self.asrAccessKeyToggle.tag == 1 ? self.asrAccessKeyField.stringValue : self.asrAccessKeySecureField.stringValue;
        yaml = yamlWrite(yaml, @"asr.access_key", accessKey);
    }

    // Update LLM fields
    if (self.llmEnabledCheckbox) {
        NSString *enabledStr = (self.llmEnabledCheckbox.state == NSControlStateValueOn) ? @"true" : @"false";
        yaml = yamlWrite(yaml, @"llm.enabled", enabledStr);
        yaml = yamlWrite(yaml, @"llm.base_url", self.llmBaseUrlField.stringValue);
        NSString *llmApiKey = self.llmApiKeyToggle.tag == 1 ? self.llmApiKeyField.stringValue : self.llmApiKeySecureField.stringValue;
        yaml = yamlWrite(yaml, @"llm.api_key", llmApiKey);
        yaml = yamlWrite(yaml, @"llm.model", self.llmModelField.stringValue);
        NSString *selectedTokenParam = self.maxTokenParamPopup.selectedItem.representedObject ?: @"max_completion_tokens";
        yaml = yamlWrite(yaml, @"llm.max_token_parameter", selectedTokenParam);
    }

    // Update hotkey
    if (self.hotkeyPopup) {
        NSString *selectedHotkey = self.hotkeyPopup.selectedItem.representedObject ?: @"fn";
        yaml = yamlWrite(yaml, @"hotkey.trigger_key", selectedHotkey);
    }

    // Write config.yaml
    NSError *error = nil;
    [yaml writeToFile:configPath atomically:YES encoding:NSUTF8StringEncoding error:&error];
    if (error) {
        NSLog(@"[Koe] Failed to write config.yaml: %@", error.localizedDescription);
        [self showAlert:@"Failed to save config.yaml" info:error.localizedDescription];
        return;
    }

    // Write dictionary.txt
    if (self.dictionaryTextView) {
        NSString *dictPath = [dir stringByAppendingPathComponent:kDictionaryFile];
        [self.dictionaryTextView.string writeToFile:dictPath atomically:YES encoding:NSUTF8StringEncoding error:&error];
        if (error) {
            NSLog(@"[Koe] Failed to write dictionary.txt: %@", error.localizedDescription);
            [self showAlert:@"Failed to save dictionary.txt" info:error.localizedDescription];
            return;
        }
    }

    // Write system_prompt.txt
    if (self.systemPromptTextView) {
        NSString *promptPath = [dir stringByAppendingPathComponent:kSystemPromptFile];
        [self.systemPromptTextView.string writeToFile:promptPath atomically:YES encoding:NSUTF8StringEncoding error:&error];
        if (error) {
            NSLog(@"[Koe] Failed to write system_prompt.txt: %@", error.localizedDescription);
            [self showAlert:@"Failed to save system_prompt.txt" info:error.localizedDescription];
            return;
        }
    }

    NSLog(@"[Koe] Settings saved");

    // Notify delegate to reload
    if ([self.delegate respondsToSelector:@selector(setupWizardDidSaveConfig)]) {
        [self.delegate setupWizardDidSaveConfig];
    }

    [self.window close];
}

- (void)cancelSetup:(id)sender {
    [self.window close];
}

- (void)llmEnabledToggled:(id)sender {
    [self updateLlmFieldsEnabled];
}

- (void)updateLlmFieldsEnabled {
    BOOL enabled = (self.llmEnabledCheckbox.state == NSControlStateValueOn);
    self.llmBaseUrlField.enabled = enabled;
    self.llmApiKeyField.enabled = enabled;
    self.llmModelField.enabled = enabled;
    self.maxTokenParamPopup.enabled = enabled;
    self.llmTestButton.enabled = enabled;
}

- (void)testLlmConnection:(id)sender {
    NSString *baseUrl = self.llmBaseUrlField.stringValue;
    NSString *apiKey = self.llmApiKeyToggle.tag == 1 ? self.llmApiKeyField.stringValue : self.llmApiKeySecureField.stringValue;
    NSString *model = self.llmModelField.stringValue;

    if (baseUrl.length == 0 || apiKey.length == 0 || model.length == 0) {
        self.llmTestResultLabel.stringValue = @"Please fill in all fields first.";
        self.llmTestResultLabel.textColor = [NSColor systemOrangeColor];
        return;
    }

    self.llmTestButton.enabled = NO;
    self.llmTestResultLabel.stringValue = @"Testing...";
    self.llmTestResultLabel.textColor = [NSColor secondaryLabelColor];

    NSString *endpoint = [baseUrl stringByTrimmingCharactersInSet:[NSCharacterSet characterSetWithCharactersInString:@"/"]];
    endpoint = [endpoint stringByAppendingString:@"/chat/completions"];
    NSURL *url = [NSURL URLWithString:endpoint];
    if (!url) {
        self.llmTestResultLabel.stringValue = @"Invalid Base URL.";
        self.llmTestResultLabel.textColor = [NSColor systemRedColor];
        self.llmTestButton.enabled = YES;
        return;
    }

    NSString *tokenParam = self.maxTokenParamPopup.selectedItem.representedObject ?: @"max_completion_tokens";
    NSDictionary *body = @{
        @"model": model,
        @"messages": @[@{@"role": @"user", @"content": @"Hi"}],
        tokenParam: @(10),
    };
    NSData *jsonData = [NSJSONSerialization dataWithJSONObject:body options:0 error:nil];

    NSMutableURLRequest *request = [NSMutableURLRequest requestWithURL:url];
    request.HTTPMethod = @"POST";
    request.HTTPBody = jsonData;
    [request setValue:@"application/json" forHTTPHeaderField:@"Content-Type"];
    [request setValue:[NSString stringWithFormat:@"Bearer %@", apiKey] forHTTPHeaderField:@"Authorization"];
    request.timeoutInterval = 15;

    NSURLSessionDataTask *task = [[NSURLSession sharedSession] dataTaskWithRequest:request
        completionHandler:^(NSData *data, NSURLResponse *response, NSError *error) {
        dispatch_async(dispatch_get_main_queue(), ^{
            self.llmTestButton.enabled = (self.llmEnabledCheckbox.state == NSControlStateValueOn);

            if (error) {
                self.llmTestResultLabel.stringValue = error.localizedDescription;
                self.llmTestResultLabel.textColor = [NSColor systemRedColor];
                return;
            }

            NSHTTPURLResponse *httpResponse = (NSHTTPURLResponse *)response;
            if (httpResponse.statusCode >= 200 && httpResponse.statusCode < 300) {
                self.llmTestResultLabel.stringValue = @"Connection successful!";
                self.llmTestResultLabel.textColor = [NSColor systemGreenColor];
            } else {
                NSString *errMsg = nil;
                if (data) {
                    NSDictionary *json = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
                    if ([json isKindOfClass:[NSDictionary class]]) {
                        NSDictionary *errObj = json[@"error"];
                        if ([errObj isKindOfClass:[NSDictionary class]]) {
                            errMsg = errObj[@"message"];
                        }
                    }
                }
                NSString *bodyStr = data ? [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] : @"";
                self.llmTestResultLabel.stringValue = [NSString stringWithFormat:@"HTTP %ld: %@",
                    (long)httpResponse.statusCode,
                    errMsg ?: bodyStr ?: @"Unknown error"];
                self.llmTestResultLabel.textColor = [NSColor systemRedColor];
            }
        });
    }];
    [task resume];
}

- (void)showAlert:(NSString *)message info:(NSString *)info {
    NSAlert *alert = [[NSAlert alloc] init];
    alert.messageText = message;
    alert.informativeText = info ?: @"";
    alert.alertStyle = NSAlertStyleWarning;
    [alert addButtonWithTitle:@"OK"];
    [alert runModal];
}

@end
