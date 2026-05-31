#import <Foundation/Foundation.h>

@interface SPPasteManager : NSObject

/// Simulate Cmd+V paste via CGEvent injection.
/// `isValid` is evaluated immediately before each delayed injection step; if it
/// returns NO (e.g. a newer session has superseded this one) the injection is
/// skipped. Pass nil to always inject. The completion block is called after a
/// short delay to allow the paste to take effect.
- (void)simulatePasteWithValidator:(BOOL (^)(void))isValid completion:(void (^)(void))completion;

/// Type text into the current focused input using Unicode keyboard events.
/// `isValid` is evaluated immediately before injection; if it returns NO the
/// injection is skipped. Pass nil to always inject. The completion block is
/// called after a short delay to allow the target app to process input.
- (void)simulateTypingText:(NSString *)text validator:(BOOL (^)(void))isValid completion:(void (^)(void))completion;

/// Simulate Cmd+Z undo, then Cmd+V paste. Used to replace previously pasted text.
/// The completion block is called after the paste takes effect.
- (void)simulateUndoThenPasteWithCompletion:(void (^)(void))completion;

/// Cancel any scheduled paste/undo blocks. Called on quit so that pending
/// CGEventPost injections cannot leak into the user's target app after the
/// hotkey monitor and event tap have been torn down.
- (void)cancel;

@end
