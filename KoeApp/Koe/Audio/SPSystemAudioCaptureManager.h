#import <Foundation/Foundation.h>

#import "SPAudioCaptureManager.h"

NS_ASSUME_NONNULL_BEGIN

typedef void (^SPSystemAudioCaptureInterruptionHandler)(NSError *error);

@interface SPSystemAudioCaptureManager : NSObject <SPTranslationAudioSource>

@property (nonatomic, copy, nullable) SPSystemAudioCaptureInterruptionHandler interruptionHandler;

@end

NS_ASSUME_NONNULL_END
