#import <Cocoa/Cocoa.h>

NS_ASSUME_NONNULL_BEGIN

@interface SPUpdateManager : NSObject

- (instancetype)initWithBundle:(NSBundle *)bundle;
- (void)start;
- (void)checkForUpdatesFromUserAction;

@end

NS_ASSUME_NONNULL_END
