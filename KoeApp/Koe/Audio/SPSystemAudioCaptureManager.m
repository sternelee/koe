#import "SPSystemAudioCaptureManager.h"

#import <CoreMedia/CoreMedia.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <float.h>
#import <math.h>
#import <mach/mach_time.h>

static NSString *const SPSystemAudioCaptureErrorDomain = @"SPSystemAudioCaptureErrorDomain";
static const double SPSystemAudioCaptureTargetSampleRate = 16000.0;
static const NSUInteger SPSystemAudioCaptureFrameBytes = 6400;
static const uint64_t SPSystemAudioCaptureFrameDurationNanos = 200000000ULL;

static float SPInterpolate(float lower, float upper, double fraction);
static int16_t SPFloatToInt16(float sample);
static uint64_t SPCurrentHostTimeInNanos(void);

@interface SPSystemAudioCaptureManager () <SCStreamDelegate, SCStreamOutput>

@property (nonatomic, strong) dispatch_queue_t sampleHandlerQueue;
@property (nonatomic, strong) NSMutableData *pendingPCM;
@property (nonatomic, copy, nullable) SPAudioFrameCallback audioCallback;
@property (nonatomic, strong, nullable) SCStream *stream;
@property (nonatomic, assign) BOOL isCapturing;
@property (nonatomic, assign) NSUInteger captureToken;
@property (nonatomic, assign) double sourceSampleRate;
@property (nonatomic, assign) double nextOutputSourceFrame;
@property (nonatomic, assign) uint64_t sourceFramesProcessed;
@property (nonatomic, assign) float previousMonoSample;
@property (nonatomic, assign) BOOL hasPreviousMonoSample;

@end

@implementation SPSystemAudioCaptureManager

- (instancetype)init {
    self = [super init];
    if (self) {
        _sampleHandlerQueue = dispatch_queue_create("com.koe.translation.system-audio", DISPATCH_QUEUE_SERIAL);
        _pendingPCM = [NSMutableData data];
    }
    return self;
}

- (void)prepareTranslationCaptureWithDeviceID:(AudioDeviceID)deviceID {
    (void)deviceID;
}

- (void)startTranslationCaptureWithAudioCallback:(SPAudioFrameCallback)callback
                                      completion:(SPTranslationCaptureStartCompletion)completion {
    @synchronized(self) {
        if (self.stream != nil || self.isCapturing) {
            if (completion) {
                completion(NO, [NSError errorWithDomain:SPSystemAudioCaptureErrorDomain
                                                   code:1
                                               userInfo:@{
                    NSLocalizedDescriptionKey: @"System audio translation capture is already active.",
                }]);
            }
            return;
        }

        self.captureToken += 1;
        self.audioCallback = callback;
        [self.pendingPCM setLength:0];
        [self resetResamplerLocked];
    }

    NSUInteger captureToken = self.captureToken;
    __weak typeof(self) weakSelf = self;
    [SCShareableContent getShareableContentExcludingDesktopWindows:NO
                                              onScreenWindowsOnly:YES
                                                completionHandler:^(SCShareableContent * _Nullable content,
                                                                    NSError * _Nullable error) {
        __strong typeof(weakSelf) self = weakSelf;
        if (!self) {
            return;
        }

        dispatch_async(dispatch_get_main_queue(), ^{
            [self handleShareableContent:content
                                   error:error
                              captureToken:captureToken
                               completion:completion];
        });
    }];
}

- (void)stopTranslationCaptureWithCompletion:(SPTranslationCaptureStopCompletion)completion {
    SPAudioFrameCallback callback = nil;
    SCStream *stream = nil;
    NSUInteger captureToken = 0;
    BOOL wasCapturing = NO;

    @synchronized(self) {
        self.captureToken += 1;
        captureToken = self.captureToken;
        callback = [self.audioCallback copy];
        self.audioCallback = nil;
        stream = self.stream;
        self.stream = nil;
        wasCapturing = self.isCapturing;
        self.isCapturing = NO;
        if (!wasCapturing && stream == nil) {
            [self flushPendingFramesLockedWithCallback:callback finalFlush:YES];
            [self.pendingPCM setLength:0];
            [self resetResamplerLocked];
        }
    }

    if (stream == nil) {
        if (completion) {
            completion();
        }
        return;
    }

    __weak typeof(self) weakSelf = self;
    [stream stopCaptureWithCompletionHandler:^(NSError * _Nullable error) {
        __strong typeof(weakSelf) self = weakSelf;
        if (!self) {
            if (completion) {
                dispatch_async(dispatch_get_main_queue(), ^{
                    completion();
                });
            }
            return;
        }

        if (error) {
            NSLog(@"[SystemAudio] Failed to stop capture cleanly: %@", error.localizedDescription);
        }

        @synchronized(self) {
            if (captureToken == self.captureToken) {
                [self flushPendingFramesLockedWithCallback:callback finalFlush:YES];
                [self.pendingPCM setLength:0];
                [self resetResamplerLocked];
            }
        }

        if (completion) {
            dispatch_async(dispatch_get_main_queue(), ^{
                completion();
            });
        }
    }];
}

- (void)stream:(SCStream *)stream
didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer
        ofType:(SCStreamOutputType)type {
    if (type != SCStreamOutputTypeAudio || sampleBuffer == NULL || !CMSampleBufferIsValid(sampleBuffer)) {
        return;
    }

    SPAudioFrameCallback callback = nil;
    uint64_t timestamp = SPCurrentHostTimeInNanos();

    @synchronized(self) {
        if (!self.isCapturing || stream != self.stream || self.audioCallback == nil) {
            return;
        }

        callback = [self.audioCallback copy];
        if (![self appendSampleBufferLocked:sampleBuffer]) {
            return;
        }
        [self flushPendingFramesLockedWithCallback:callback finalFlush:NO baseTimestamp:timestamp];
    }
}

- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    NSLog(@"[SystemAudio] Capture stopped: %@", error.localizedDescription);
    SPSystemAudioCaptureInterruptionHandler interruptionHandler = nil;
    @synchronized(self) {
        if (stream == self.stream) {
            self.stream = nil;
            self.isCapturing = NO;
            self.audioCallback = nil;
            [self.pendingPCM setLength:0];
            [self resetResamplerLocked];
            interruptionHandler = [self.interruptionHandler copy];
        }
    }
    if (interruptionHandler) {
        dispatch_async(dispatch_get_main_queue(), ^{
            interruptionHandler(error);
        });
    }
}

#pragma mark - Internal

- (void)handleShareableContent:(SCShareableContent * _Nullable)content
                         error:(NSError * _Nullable)error
                    captureToken:(NSUInteger)captureToken
                     completion:(SPTranslationCaptureStartCompletion)completion {
    if (error) {
        [self finishStartForCaptureToken:captureToken
                                 started:NO
                                   error:error
                              completion:completion];
        return;
    }

    SCDisplay *display = content.displays.firstObject;
    if (display == nil) {
        [self finishStartForCaptureToken:captureToken
                                 started:NO
                                   error:[NSError errorWithDomain:SPSystemAudioCaptureErrorDomain
                                                              code:2
                                                          userInfo:@{
            NSLocalizedDescriptionKey: @"No display is available for system audio capture.",
        }]
                              completion:completion];
        return;
    }

    SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:display
                                                  excludingApplications:@[]
                                                       exceptingWindows:@[]];
    SCStreamConfiguration *configuration = [[SCStreamConfiguration alloc] init];
    configuration.capturesAudio = YES;
    configuration.excludesCurrentProcessAudio = YES;
    configuration.sampleRate = (NSInteger)SPSystemAudioCaptureTargetSampleRate;
    configuration.channelCount = 1;
    configuration.width = 2;
    configuration.height = 2;
    configuration.minimumFrameInterval = CMTimeMake(1, 1);
    configuration.showsCursor = NO;

    SCStream *stream = [[SCStream alloc] initWithFilter:filter
                                          configuration:configuration
                                               delegate:self];
    NSError *outputError = nil;
    [stream addStreamOutput:self
                       type:SCStreamOutputTypeAudio
         sampleHandlerQueue:self.sampleHandlerQueue
                      error:&outputError];
    if (outputError) {
        [self finishStartForCaptureToken:captureToken
                                 started:NO
                                   error:outputError
                              completion:completion];
        return;
    }

    @synchronized(self) {
        if (captureToken != self.captureToken || self.audioCallback == nil) {
            [self safelyStopStream:stream completion:nil];
            if (completion) {
                completion(NO, [NSError errorWithDomain:NSCocoaErrorDomain
                                                   code:NSUserCancelledError
                                               userInfo:nil]);
            }
            return;
        }
        self.stream = stream;
    }

    __weak typeof(self) weakSelf = self;
    [stream startCaptureWithCompletionHandler:^(NSError * _Nullable error) {
        __strong typeof(weakSelf) self = weakSelf;
        if (!self) {
            return;
        }

        dispatch_async(dispatch_get_main_queue(), ^{
            if (error) {
                @synchronized(self) {
                    if (captureToken == self.captureToken && self.stream == stream) {
                        self.stream = nil;
                        self.audioCallback = nil;
                    }
                }
                [self finishStartForCaptureToken:captureToken
                                         started:NO
                                           error:error
                                      completion:completion];
                return;
            }

            BOOL staleStart = NO;
            @synchronized(self) {
                staleStart = (captureToken != self.captureToken || self.audioCallback == nil || self.stream != stream);
                if (!staleStart) {
                    self.isCapturing = YES;
                }
            }

            if (staleStart) {
                [self safelyStopStream:stream completion:nil];
                if (completion) {
                    completion(NO, [NSError errorWithDomain:NSCocoaErrorDomain
                                                       code:NSUserCancelledError
                                                   userInfo:nil]);
                }
                return;
            }

            if (completion) {
                completion(YES, nil);
            }
        });
    }];
}

- (void)finishStartForCaptureToken:(NSUInteger)captureToken
                           started:(BOOL)started
                             error:(NSError * _Nullable)error
                        completion:(SPTranslationCaptureStartCompletion)completion {
    @synchronized(self) {
        if (captureToken == self.captureToken && !started) {
            self.stream = nil;
            self.isCapturing = NO;
            self.audioCallback = nil;
            [self.pendingPCM setLength:0];
            [self resetResamplerLocked];
        }
    }

    if (completion) {
        completion(started, error);
    }
}

- (BOOL)appendSampleBufferLocked:(CMSampleBufferRef)sampleBuffer {
    CMAudioFormatDescriptionRef formatDescription = CMSampleBufferGetFormatDescription(sampleBuffer);
    const AudioStreamBasicDescription *asbd = CMAudioFormatDescriptionGetStreamBasicDescription(formatDescription);
    if (asbd == NULL || asbd->mFormatID != kAudioFormatLinearPCM) {
        NSLog(@"[SystemAudio] Unsupported audio format from ScreenCaptureKit.");
        return NO;
    }

    NSUInteger frameCount = (NSUInteger)CMSampleBufferGetNumSamples(sampleBuffer);
    if (frameCount == 0) {
        return YES;
    }

    size_t bufferListSize = offsetof(AudioBufferList, mBuffers) + sizeof(AudioBuffer) * MAX((uint32_t)1, asbd->mChannelsPerFrame);
    AudioBufferList *bufferList = malloc(bufferListSize);
    if (bufferList == NULL) {
        return NO;
    }

    CMBlockBufferRef blockBuffer = NULL;
    OSStatus status = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
        sampleBuffer,
        NULL,
        bufferList,
        bufferListSize,
        kCFAllocatorDefault,
        kCFAllocatorDefault,
        kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment,
        &blockBuffer
    );
    if (status != noErr) {
        free(bufferList);
        if (blockBuffer != NULL) {
            CFRelease(blockBuffer);
        }
        NSLog(@"[SystemAudio] Failed to access audio buffer list: %d", (int)status);
        return NO;
    }

    BOOL isFloat = ((asbd->mFormatFlags & kAudioFormatFlagIsFloat) != 0) && asbd->mBitsPerChannel == 32;
    BOOL isSignedInt16 = ((asbd->mFormatFlags & kAudioFormatFlagIsSignedInteger) != 0) && asbd->mBitsPerChannel == 16;
    BOOL isInterleaved = (asbd->mFormatFlags & kAudioFormatFlagIsNonInterleaved) == 0;
    NSUInteger channelCount = MAX((NSUInteger)1, (NSUInteger)asbd->mChannelsPerFrame);

    if (!isFloat && !isSignedInt16) {
        free(bufferList);
        if (blockBuffer != NULL) {
            CFRelease(blockBuffer);
        }
        NSLog(@"[SystemAudio] Unsupported PCM bit depth: %u", (unsigned)asbd->mBitsPerChannel);
        return NO;
    }

    NSMutableData *monoData = [NSMutableData dataWithLength:frameCount * sizeof(float)];
    float *monoSamples = monoData.mutableBytes;

    if (isFloat) {
        if (isInterleaved) {
            const float *samples = bufferList->mBuffers[0].mData;
            for (NSUInteger frameIndex = 0; frameIndex < frameCount; frameIndex += 1) {
                float sum = 0.0f;
                NSUInteger baseIndex = frameIndex * channelCount;
                for (NSUInteger channelIndex = 0; channelIndex < channelCount; channelIndex += 1) {
                    sum += samples[baseIndex + channelIndex];
                }
                monoSamples[frameIndex] = sum / (float)channelCount;
            }
        } else {
            NSUInteger availableBuffers = MIN(channelCount, (NSUInteger)bufferList->mNumberBuffers);
            for (NSUInteger frameIndex = 0; frameIndex < frameCount; frameIndex += 1) {
                float sum = 0.0f;
                for (NSUInteger channelIndex = 0; channelIndex < availableBuffers; channelIndex += 1) {
                    const float *channelSamples = bufferList->mBuffers[channelIndex].mData;
                    sum += channelSamples[frameIndex];
                }
                monoSamples[frameIndex] = sum / (float)availableBuffers;
            }
        }
    } else {
        if (isInterleaved) {
            const int16_t *samples = bufferList->mBuffers[0].mData;
            for (NSUInteger frameIndex = 0; frameIndex < frameCount; frameIndex += 1) {
                float sum = 0.0f;
                NSUInteger baseIndex = frameIndex * channelCount;
                for (NSUInteger channelIndex = 0; channelIndex < channelCount; channelIndex += 1) {
                    sum += (float)samples[baseIndex + channelIndex] / 32768.0f;
                }
                monoSamples[frameIndex] = sum / (float)channelCount;
            }
        } else {
            NSUInteger availableBuffers = MIN(channelCount, (NSUInteger)bufferList->mNumberBuffers);
            for (NSUInteger frameIndex = 0; frameIndex < frameCount; frameIndex += 1) {
                float sum = 0.0f;
                for (NSUInteger channelIndex = 0; channelIndex < availableBuffers; channelIndex += 1) {
                    const int16_t *channelSamples = bufferList->mBuffers[channelIndex].mData;
                    sum += (float)channelSamples[frameIndex] / 32768.0f;
                }
                monoSamples[frameIndex] = sum / (float)availableBuffers;
            }
        }
    }

    free(bufferList);
    if (blockBuffer != NULL) {
        CFRelease(blockBuffer);
    }

    [self appendMonoSamplesLocked:monoSamples
                            count:frameCount
                       sampleRate:asbd->mSampleRate];
    return YES;
}

- (void)appendMonoSamplesLocked:(const float *)samples
                          count:(NSUInteger)count
                     sampleRate:(double)sampleRate {
    if (samples == NULL || count == 0 || sampleRate <= 0.0) {
        return;
    }

    if (self.sourceSampleRate <= 0.0 || fabs(self.sourceSampleRate - sampleRate) > 0.5) {
        self.sourceSampleRate = sampleRate;
        self.nextOutputSourceFrame = 0.0;
        self.sourceFramesProcessed = 0;
        self.hasPreviousMonoSample = NO;
    }

    double step = self.sourceSampleRate / SPSystemAudioCaptureTargetSampleRate;
    double chunkStart = (double)self.sourceFramesProcessed;
    double maxInterpolatedSourceFrame = chunkStart + (double)count - 1.0;
    NSMutableData *convertedPCM = [NSMutableData data];

    while (self.nextOutputSourceFrame <= maxInterpolatedSourceFrame) {
        float outputSample = 0.0f;
        if (self.nextOutputSourceFrame < chunkStart) {
            if (!self.hasPreviousMonoSample) {
                self.nextOutputSourceFrame = chunkStart;
                continue;
            }
            double fraction = self.nextOutputSourceFrame - (chunkStart - 1.0);
            outputSample = SPInterpolate(self.previousMonoSample, samples[0], fraction);
        } else {
            double localPosition = self.nextOutputSourceFrame - chunkStart;
            NSUInteger lowerIndex = (NSUInteger)floor(localPosition);
            double fraction = localPosition - (double)lowerIndex;
            if (fraction <= DBL_EPSILON || lowerIndex + 1 >= count) {
                outputSample = samples[lowerIndex];
            } else {
                outputSample = SPInterpolate(samples[lowerIndex], samples[lowerIndex + 1], fraction);
            }
        }

        int16_t pcmSample = SPFloatToInt16(outputSample);
        [convertedPCM appendBytes:&pcmSample length:sizeof(pcmSample)];
        self.nextOutputSourceFrame += step;
    }

    [self.pendingPCM appendData:convertedPCM];
    self.previousMonoSample = samples[count - 1];
    self.hasPreviousMonoSample = YES;
    self.sourceFramesProcessed += count;
}

- (void)flushPendingFramesLockedWithCallback:(SPAudioFrameCallback)callback finalFlush:(BOOL)finalFlush {
    [self flushPendingFramesLockedWithCallback:callback
                                     finalFlush:finalFlush
                                  baseTimestamp:SPCurrentHostTimeInNanos()];
}

- (void)flushPendingFramesLockedWithCallback:(SPAudioFrameCallback)callback
                                   finalFlush:(BOOL)finalFlush
                                baseTimestamp:(uint64_t)baseTimestamp {
    if (callback == nil) {
        [self.pendingPCM setLength:0];
        return;
    }

    NSUInteger frameIndex = 0;
    while (self.pendingPCM.length >= SPSystemAudioCaptureFrameBytes) {
        NSData *frame = [self.pendingPCM subdataWithRange:NSMakeRange(0, SPSystemAudioCaptureFrameBytes)];
        callback(frame.bytes,
                 (uint32_t)frame.length,
                 baseTimestamp + (uint64_t)frameIndex * SPSystemAudioCaptureFrameDurationNanos);
        [self.pendingPCM replaceBytesInRange:NSMakeRange(0, SPSystemAudioCaptureFrameBytes)
                                   withBytes:NULL
                                      length:0];
        frameIndex += 1;
    }

    if (finalFlush && self.pendingPCM.length > 0) {
        NSData *frame = [self.pendingPCM copy];
        callback(frame.bytes, (uint32_t)frame.length, baseTimestamp);
        [self.pendingPCM setLength:0];
    }
}

- (void)resetResamplerLocked {
    self.sourceSampleRate = 0.0;
    self.nextOutputSourceFrame = 0.0;
    self.sourceFramesProcessed = 0;
    self.previousMonoSample = 0.0f;
    self.hasPreviousMonoSample = NO;
}

- (void)safelyStopStream:(SCStream *)stream completion:(dispatch_block_t _Nullable)completion {
    [stream stopCaptureWithCompletionHandler:^(NSError * _Nullable error) {
        if (error) {
            NSLog(@"[SystemAudio] Failed to cancel stale capture: %@", error.localizedDescription);
        }
        if (completion) {
            dispatch_async(dispatch_get_main_queue(), ^{
                completion();
            });
        }
    }];
}

static float SPInterpolate(float lower, float upper, double fraction) {
    return (float)(lower + (upper - lower) * fraction);
}

static int16_t SPFloatToInt16(float sample) {
    float clamped = fmaxf(-1.0f, fminf(1.0f, sample));
    return (int16_t)lrintf(clamped * 32767.0f);
}

static uint64_t SPCurrentHostTimeInNanos(void) {
    return AudioConvertHostTimeToNanos(mach_absolute_time());
}

@end
