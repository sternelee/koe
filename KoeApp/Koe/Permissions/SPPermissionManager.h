#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

typedef void (^SPPermissionCheckCompletion)(BOOL microphoneGranted,
                                            BOOL accessibilityGranted,
                                            BOOL inputMonitoringGranted);

typedef NS_ENUM(NSInteger, SPPermissionType) {
    SPPermissionTypeMicrophone = 0,
    SPPermissionTypeAccessibility,
    SPPermissionTypeInputMonitoring,
    SPPermissionTypeSpeechRecognition,
    SPPermissionTypeScreenRecording,
};

@interface SPPermissionManager : NSObject

+ (instancetype)sharedManager;

- (void)checkAllPermissionsWithCompletion:(SPPermissionCheckCompletion)completion;
- (BOOL)isMicrophoneGranted;
- (BOOL)requestMicrophonePermission;
- (BOOL)isAccessibilityGranted;
- (BOOL)requestAccessibilityPermission;
- (BOOL)isInputMonitoringGranted;
- (BOOL)requestInputMonitoringPermission;
- (BOOL)isSpeechRecognitionGranted;
- (void)requestSpeechRecognitionPermissionWithCompletion:(void (^)(BOOL granted))completion;
- (void)checkNotificationPermissionWithCompletion:(void (^)(BOOL granted))completion;
- (void)requestNotificationPermission;
- (void)requestNotificationPermissionWithCompletion:(void (^)(BOOL granted))completion;
- (BOOL)isScreenRecordingGranted;
- (BOOL)requestScreenRecordingPermission;
- (BOOL)showPermissionAlertForType:(SPPermissionType)type settingsURL:(nullable NSURL *)settingsURL;

@end

NS_ASSUME_NONNULL_END
