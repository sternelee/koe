#import "SPAppDelegate.h"
#import "SPPermissionManager.h"
#import "SPHotkeyMonitor.h"
#import "SPAudioCaptureManager.h"
#import "SPRustBridge.h"
#import "SPClipboardManager.h"
#import "SPPasteManager.h"
#import "SPCuePlayer.h"
#import "SPStatusBarManager.h"
#import "SPHistoryManager.h"

@interface SPAppDelegate ()
@property (nonatomic, strong) NSDate *recordingStartTime;
@end

@implementation SPAppDelegate

- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    NSLog(@"[Koe] Application launching...");

    // Initialize components
    self.cuePlayer = [[SPCuePlayer alloc] init];
    self.clipboardManager = [[SPClipboardManager alloc] init];
    self.pasteManager = [[SPPasteManager alloc] init];
    self.audioCaptureManager = [[SPAudioCaptureManager alloc] init];
    self.permissionManager = [[SPPermissionManager alloc] init];

    // Initialize Rust bridge (must be before hotkey monitor)
    self.rustBridge = [[SPRustBridge alloc] initWithDelegate:self];
    [self.rustBridge initializeCore];

    // Initialize status bar
    self.statusBarManager = [[SPStatusBarManager alloc] initWithDelegate:self
                                                       permissionManager:self.permissionManager];

    // Check permissions
    [self.permissionManager checkAllPermissionsWithCompletion:^(BOOL micGranted, BOOL accessibilityGranted, BOOL inputMonitoringGranted) {
        NSLog(@"[Koe] Permissions — mic:%d accessibility:%d inputMonitoring:%d",
              micGranted, accessibilityGranted, inputMonitoringGranted);

        if (!micGranted) {
            NSLog(@"[Koe] ERROR: Microphone permission not granted");
            [self.cuePlayer playError];
            return;
        }

        if (!inputMonitoringGranted) {
            NSLog(@"[Koe] WARNING: Input Monitoring probe failed, will attempt hotkey monitor anyway");
        }

        // Start hotkey monitor (let it try CGEventTap directly — the probe may give false negatives)
        self.hotkeyMonitor = [[SPHotkeyMonitor alloc] initWithDelegate:self];
        [self.hotkeyMonitor start];
        NSLog(@"[Koe] Ready — hotkey monitor active");
    }];
}

- (void)applicationWillTerminate:(NSNotification *)notification {
    NSLog(@"[Koe] Application terminating...");
    [self.hotkeyMonitor stop];
    [self.rustBridge destroyCore];
}

#pragma mark - SPHotkeyMonitorDelegate

- (void)hotkeyMonitorDidDetectHoldStart {
    NSLog(@"[Koe] Hold start detected");
    self.recordingStartTime = [NSDate date];
    [self.cuePlayer reloadFeedbackConfig];
    [self.cuePlayer playStart];
    [self.statusBarManager updateState:@"recording"];

    // Start audio capture + Rust session
    [self.rustBridge beginSessionWithMode:SPSessionModeHold];
    [self.audioCaptureManager startCaptureWithAudioCallback:^(const void *buffer, uint32_t length, uint64_t timestamp) {
        [self.rustBridge pushAudioFrame:buffer length:length timestamp:timestamp];
    }];
}

- (void)hotkeyMonitorDidDetectHoldEnd {
    NSLog(@"[Koe] Hold end detected");
    [self.cuePlayer playStop];

    // Keep recording for 800ms after Fn release to capture trailing speech,
    // then stop mic and end session
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(300 * NSEC_PER_MSEC)),
                   dispatch_get_main_queue(), ^{
        [self.audioCaptureManager stopCapture];
        [self.rustBridge endSession];
    });
}

- (void)hotkeyMonitorDidDetectTapStart {
    NSLog(@"[Koe] Tap start detected");
    self.recordingStartTime = [NSDate date];
    [self.cuePlayer reloadFeedbackConfig];
    [self.cuePlayer playStart];
    [self.statusBarManager updateState:@"recording"];

    [self.rustBridge beginSessionWithMode:SPSessionModeToggle];
    [self.audioCaptureManager startCaptureWithAudioCallback:^(const void *buffer, uint32_t length, uint64_t timestamp) {
        [self.rustBridge pushAudioFrame:buffer length:length timestamp:timestamp];
    }];
}

- (void)hotkeyMonitorDidDetectTapEnd {
    NSLog(@"[Koe] Tap end detected");
    [self.cuePlayer playStop];

    // Keep recording for 800ms after tap-end to capture trailing speech,
    // then stop mic and end session
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(300 * NSEC_PER_MSEC)),
                   dispatch_get_main_queue(), ^{
        [self.audioCaptureManager stopCapture];
        [self.rustBridge endSession];
    });
}

#pragma mark - SPRustBridgeDelegate

- (void)rustBridgeDidBecomeReady {
    NSLog(@"[Koe] Session ready (ASR connected)");
}

- (void)rustBridgeDidReceiveFinalText:(NSString *)text {
    NSLog(@"[Koe] Final text received (%lu chars)", (unsigned long)text.length);

    // Record history
    NSInteger durationMs = 0;
    if (self.recordingStartTime) {
        durationMs = (NSInteger)(-[self.recordingStartTime timeIntervalSinceNow] * 1000);
        self.recordingStartTime = nil;
    }
    [[SPHistoryManager sharedManager] recordSessionWithDurationMs:durationMs text:text];

    [self.statusBarManager updateState:@"pasting"];

    // Backup clipboard, write text, paste, restore
    [self.clipboardManager backup];
    [self.clipboardManager writeText:text];

    // Check if accessibility is available for auto-paste
    if ([self.permissionManager isAccessibilityGranted]) {
        [self.pasteManager simulatePasteWithCompletion:^{
            [self.clipboardManager scheduleRestoreAfterDelay:1500];
            [self.statusBarManager updateState:@"idle"];
        }];
    } else {
        NSLog(@"[Koe] Accessibility not granted — text copied to clipboard only");
        [self.statusBarManager updateState:@"idle"];
    }
}

- (void)rustBridgeDidEncounterError:(NSString *)message {
    NSLog(@"[Koe] Session error: %@", message);
    [self.cuePlayer playError];
    [self.audioCaptureManager stopCapture];
    [self.statusBarManager updateState:@"error"];

    // Brief error display, then back to idle
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(2 * NSEC_PER_SEC)),
                   dispatch_get_main_queue(), ^{
        [self.statusBarManager updateState:@"idle"];
    });
}

- (void)rustBridgeDidChangeState:(NSString *)state {
    [self.statusBarManager updateState:state];
}

@end
