#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

/// Installs and uninstalls the KoeVirtualMic CoreAudio HAL plug-in into the
/// system-wide /Library/Audio/Plug-Ins/HAL directory. Both operations require
/// administrator privileges and are gated behind an AppleScript prompt.
@interface SPVirtualMicInstaller : NSObject

/// Returns YES when the driver bundle exists at the system HAL path.
+ (BOOL)isInstalled;

/// Locates the driver bundle to install from. Searches the host app's
/// Resources directory first; returns nil if no bundle is found.
+ (nullable NSString *)findBundlePath;

/// Copies the driver bundle into /Library/Audio/Plug-Ins/HAL and reloads
/// coreaudiod so the new device shows up. Calls `completion` on the main
/// queue with `nil` on success or an `NSError *` describing the failure.
+ (void)installWithCompletion:(void (^)(NSError * _Nullable error))completion;

/// Removes the driver bundle from the system HAL directory and reloads
/// coreaudiod. Calls `completion` on the main queue.
+ (void)uninstallWithCompletion:(void (^)(NSError * _Nullable error))completion;

@end

NS_ASSUME_NONNULL_END
