#import "SPVirtualMicInstaller.h"

#import <AppKit/AppKit.h>

static NSString *const kVirtualMicHalDir = @"/Library/Audio/Plug-Ins/HAL";
static NSString *const kVirtualMicBundleName = @"KoeVirtualMic.driver";
static NSString *const kVirtualMicErrorDomain = @"nz.owo.koe.virtual-mic-installer";

typedef NS_ENUM(NSInteger, SPVirtualMicInstallerErrorCode) {
    SPVirtualMicInstallerErrorBundleNotFound = 1,
    SPVirtualMicInstallerErrorAppleScriptCreationFailed,
    SPVirtualMicInstallerErrorAppleScriptFailed,
    SPVirtualMicInstallerErrorReloadFailed,
};

@implementation SPVirtualMicInstaller

+ (NSString *)installedPath {
    return [kVirtualMicHalDir stringByAppendingPathComponent:kVirtualMicBundleName];
}

+ (BOOL)isInstalled {
    return [[NSFileManager defaultManager] fileExistsAtPath:[self installedPath]];
}

+ (nullable NSString *)findBundlePath {
    NSFileManager *fm = [NSFileManager defaultManager];
    for (NSString *candidate in [self bundleCandidates]) {
        if ([fm fileExistsAtPath:candidate]) {
            return candidate;
        }
    }
    return nil;
}

+ (void)installWithCompletion:(void (^)(NSError * _Nullable))completion {
    NSString *sourcePath = [self findBundlePath];
    if (sourcePath == nil) {
        [self callCompletion:completion withError:[self errorWithCode:SPVirtualMicInstallerErrorBundleNotFound
                                                              message:@"KoeVirtualMic.driver not found inside Koe.app. Try rebuilding the app."]];
        return;
    }

    NSString *installedPath = [self installedPath];
    NSString *escapedSource = [self escapeForShell:sourcePath];
    NSString *escapedTarget = [self escapeForShell:installedPath];
    NSString *script = [NSString stringWithFormat:
        @"do shell script \"mkdir -p %@ && rm -rf %@ && cp -R %@ %@\" with administrator privileges",
        kVirtualMicHalDir, escapedTarget, escapedSource, escapedTarget];

    [self runAppleScript:script completion:^(NSError * _Nullable scriptError) {
        if (scriptError != nil) {
            [self callCompletion:completion withError:scriptError];
            return;
        }
        [self reloadCoreAudioWithCompletion:completion];
    }];
}

+ (void)uninstallWithCompletion:(void (^)(NSError * _Nullable))completion {
    NSString *escapedTarget = [self escapeForShell:[self installedPath]];
    NSString *script = [NSString stringWithFormat:
        @"do shell script \"rm -rf %@\" with administrator privileges",
        escapedTarget];

    [self runAppleScript:script completion:^(NSError * _Nullable scriptError) {
        if (scriptError != nil) {
            [self callCompletion:completion withError:scriptError];
            return;
        }
        [self reloadCoreAudioWithCompletion:completion];
    }];
}

#pragma mark - Helpers

+ (NSArray<NSString *> *)bundleCandidates {
    NSMutableArray<NSString *> *candidates = [NSMutableArray array];
    NSBundle *main = [NSBundle mainBundle];
    NSURL *resourcesURL = main.resourceURL;
    if (resourcesURL != nil) {
        [candidates addObject:[resourcesURL URLByAppendingPathComponent:kVirtualMicBundleName].path];
    }
    NSURL *executableURL = main.executableURL;
    if (executableURL != nil) {
        NSURL *executableDir = executableURL.URLByDeletingLastPathComponent;
        [candidates addObject:[executableDir URLByAppendingPathComponent:kVirtualMicBundleName].path];
        // Two levels up: Contents/MacOS/.. → Contents/.. → app-adjacent.
        NSURL *appAdjacent = executableDir.URLByDeletingLastPathComponent.URLByDeletingLastPathComponent;
        [candidates addObject:[appAdjacent URLByAppendingPathComponent:kVirtualMicBundleName].path];
    }
    return [[NSOrderedSet orderedSetWithArray:candidates] array];
}

+ (NSString *)escapeForShell:(NSString *)path {
    // Wrap the whole path in single quotes and escape any contained quote.
    NSString *escaped = [path stringByReplacingOccurrencesOfString:@"'" withString:@"'\\''"];
    return [NSString stringWithFormat:@"'%@'", escaped];
}

+ (void)runAppleScript:(NSString *)source completion:(void (^)(NSError * _Nullable))completion {
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSAppleScript *script = [[NSAppleScript alloc] initWithSource:source];
        if (script == nil) {
            completion([self errorWithCode:SPVirtualMicInstallerErrorAppleScriptCreationFailed
                                   message:@"Failed to construct AppleScript for privilege elevation."]);
            return;
        }
        NSDictionary *errorInfo = nil;
        [script executeAndReturnError:&errorInfo];
        if (errorInfo != nil) {
            NSString *message = errorInfo[@"NSAppleScriptErrorMessage"] ?: @"unknown AppleScript error";
            completion([self errorWithCode:SPVirtualMicInstallerErrorAppleScriptFailed message:message]);
            return;
        }
        completion(nil);
    });
}

+ (void)reloadCoreAudioWithCompletion:(void (^)(NSError * _Nullable))completion {
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSTask *task = [[NSTask alloc] init];
        task.executableURL = [NSURL fileURLWithPath:@"/bin/launchctl"];
        task.arguments = @[@"kickstart", @"-k", @"system/com.apple.audio.coreaudiod"];
        NSPipe *pipe = [NSPipe pipe];
        task.standardOutput = pipe;
        task.standardError = pipe;
        NSError *launchError = nil;
        if (![task launchAndReturnError:&launchError]) {
            [self callCompletion:completion withError:launchError ?: [self errorWithCode:SPVirtualMicInstallerErrorReloadFailed
                                                                                  message:@"Failed to launch launchctl."]];
            return;
        }
        [task waitUntilExit];
        if (task.terminationStatus == 0) {
            [self callCompletion:completion withError:nil];
            return;
        }
        NSData *data = [pipe.fileHandleForReading readDataToEndOfFile];
        NSString *output = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] ?:
            [NSString stringWithFormat:@"launchctl exited %d", task.terminationStatus];
        [self callCompletion:completion withError:[self errorWithCode:SPVirtualMicInstallerErrorReloadFailed message:output]];
    });
}

+ (NSError *)errorWithCode:(SPVirtualMicInstallerErrorCode)code message:(NSString *)message {
    return [NSError errorWithDomain:kVirtualMicErrorDomain
                               code:code
                           userInfo:@{NSLocalizedDescriptionKey: message ?: @"Virtual mic installer error"}];
}

+ (void)callCompletion:(void (^)(NSError * _Nullable))completion withError:(NSError * _Nullable)error {
    if (completion == nil) {
        return;
    }
    dispatch_async(dispatch_get_main_queue(), ^{
        completion(error);
    });
}

@end
