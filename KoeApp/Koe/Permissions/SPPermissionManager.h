#import <Foundation/Foundation.h>

typedef void (^SPPermissionCheckCompletion)(BOOL micGranted, BOOL accessibilityGranted, BOOL inputMonitoringGranted);

@interface SPPermissionManager : NSObject

- (void)checkAllPermissionsWithCompletion:(SPPermissionCheckCompletion)completion;
- (BOOL)isMicrophoneGranted;
- (BOOL)isAccessibilityGranted;

/// Request accessibility permission (shows system dialog). Call this when user clicks to request permission.
- (void)requestAccessibilityPermission;
- (BOOL)isInputMonitoringGranted;

/// Request notification permission from the user.
- (void)requestNotificationPermission;

/// Check whether notification permission has been granted.
/// @param completion Called on main queue with the current authorization status.
- (void)checkNotificationPermissionWithCompletion:(void (^)(BOOL granted))completion;

@end
