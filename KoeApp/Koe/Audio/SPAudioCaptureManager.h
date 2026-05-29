#import <Foundation/Foundation.h>
#import <CoreAudio/CoreAudio.h>

/// Callback invoked for each captured audio frame.
/// buffer: pointer to PCM Int16 LE data
/// length: byte length of the buffer
/// timestamp: host time in nanoseconds
typedef void (^SPAudioFrameCallback)(const void *buffer, uint32_t length, uint64_t timestamp);
typedef void (^SPTranslationCaptureStartCompletion)(BOOL started, NSError *_Nullable error);
typedef void (^SPTranslationCaptureStopCompletion)(void);

/// Translation-mode capture abstraction. The current implementation is
/// microphone-backed, but future system-playback capture can conform here
/// without AppDelegate depending on a concrete capture manager.
@protocol SPTranslationAudioSource <NSObject>

- (void)prepareTranslationCaptureWithDeviceID:(AudioDeviceID)deviceID;
- (void)startTranslationCaptureWithAudioCallback:(SPAudioFrameCallback)callback
                                      completion:(SPTranslationCaptureStartCompletion)completion;
- (void)stopTranslationCaptureWithCompletion:(SPTranslationCaptureStopCompletion _Nullable)completion;

@property (nonatomic, readonly) BOOL isCapturing;

@end

@interface SPAudioCaptureManager : NSObject <SPTranslationAudioSource>

/// Set the input device for the next capture session.
/// Must be called BEFORE startCaptureWithAudioCallback:.
/// Pass kAudioObjectUnknown (0) to use the system default input device.
- (void)setInputDeviceID:(AudioDeviceID)deviceID;

/// Start audio capture. Captured frames are delivered via the callback.
/// Audio format: 16kHz, mono, PCM Int16 LE, ~200ms per frame (3200 samples).
/// Returns YES on success, NO if capture could not be started.
- (BOOL)startCaptureWithAudioCallback:(SPAudioFrameCallback)callback;

/// Stop audio capture.
- (void)stopCapture;

@property (nonatomic, readonly) BOOL isCapturing;

@end
