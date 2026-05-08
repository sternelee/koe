#import <Cocoa/Cocoa.h>

@class SPRustBridge;
@class SPAudioDeviceManager;

@protocol SPSetupWizardDelegate <NSObject>
@optional
/// Called after the wizard saves config, so the app can reload.
- (void)setupWizardDidSaveConfig;
@end

@interface SPSetupWizardWindowController : NSWindowController

@property (nonatomic, weak) id<SPSetupWizardDelegate> delegate;
@property (nonatomic, strong) SPRustBridge *rustBridge;
@property (nonatomic, strong) SPAudioDeviceManager *audioDeviceManager;

@end
