#include "koe_virtual_mic_driver.h"

#include <CoreFoundation/CoreFoundation.h>
#include <CoreAudio/AudioHardware.h>
#include <mach/mach_time.h>
#include <stddef.h>
#include <string.h>
#include <os/log.h>

namespace {
AudioServerPlugInDriverInterface kDriverInterface = {};
struct DriverRefStorage {
    AudioServerPlugInDriverInterface *interface_ptr;
    KoeVirtualMicDriver *driver;
};

DriverRefStorage kDriverStorage = {&kDriverInterface, nullptr};

constexpr Float64 kNominalSampleRate = 48000.0;
constexpr UInt32 kChannelCount = 1;
constexpr UInt32 kZeroTimeStampPeriod = 480;
constexpr const char *kPluginName = "Koe Virtual Mic Plug-In";
constexpr const char *kDeviceName = "Koe Virtual Mic";
constexpr const char *kManufacturerName = "Koe Virtual Mic";
constexpr const char *kStreamName = "Koe Virtual Mic Stream";
constexpr const char *kDeviceUID = "koe.virtual.mic.device";
constexpr const char *kModelUID = "koe.virtual.mic.model";
constexpr const char *kResourceBundleName = "Resources";
constexpr AudioObjectPropertySelector kPropertyListenerRemoved = 'lisr';
constexpr AudioObjectPropertySelector kPropertyTapList = 'taps';

enum RenderState : UInt32 {
    kRenderStateUnavailable = 0,
    kRenderStateFormatMismatch = 1,
    kRenderStateSilence = 2,
    kRenderStateFlowing = 3,
};

CFStringRef copy_cf_string(const char *value) {
    return CFStringCreateWithCString(kCFAllocatorDefault, value, kCFStringEncodingUTF8);
}

bool addresses_match_selector(const AudioObjectPropertyAddress *address, AudioObjectPropertySelector selector) {
    return address != nullptr && address->mSelector == selector;
}

bool uuid_bytes_equal(REFIID lhs, CFUUIDRef rhs) {
    const CFUUIDBytes rhs_bytes = CFUUIDGetUUIDBytes(rhs);
    return memcmp(&lhs, &rhs_bytes, sizeof(CFUUIDBytes)) == 0;
}

constexpr UInt32 single_buffer_list_size() {
    return static_cast<UInt32>(offsetof(AudioBufferList, mBuffers) + sizeof(AudioBuffer));
}

AudioStreamBasicDescription make_mono_float_format() {
    AudioStreamBasicDescription asbd = {};
    asbd.mSampleRate = kNominalSampleRate;
    asbd.mFormatID = kAudioFormatLinearPCM;
    asbd.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked;
    asbd.mBytesPerPacket = sizeof(Float32);
    asbd.mFramesPerPacket = 1;
    asbd.mBytesPerFrame = sizeof(Float32);
    asbd.mChannelsPerFrame = kChannelCount;
    asbd.mBitsPerChannel = 32;
    return asbd;
}

bool is_input_scope(const AudioObjectPropertyAddress *address) {
    return address != nullptr &&
        (address->mScope == kAudioObjectPropertyScopeInput || address->mScope == kAudioObjectPropertyScopeGlobal);
}

bool is_output_scope(const AudioObjectPropertyAddress *address) {
    return address != nullptr && address->mScope == kAudioObjectPropertyScopeOutput;
}

bool is_stream_scope(const AudioObjectPropertyAddress *address) {
    return address != nullptr &&
        (address->mScope == kAudioObjectPropertyScopeInput ||
         address->mScope == kAudioObjectPropertyScopeOutput ||
         address->mScope == kAudioObjectPropertyScopeGlobal ||
         address->mScope == kAudioObjectPropertyScopeWildcard);
}

CFStringRef copy_element_name(const AudioObjectPropertyAddress *address) {
    if (address == nullptr) {
        return copy_cf_string("Main");
    }

    if (address->mSelector == kAudioObjectPropertyElementCategoryName) {
        if (address->mScope == kAudioObjectPropertyScopeInput) {
            return copy_cf_string("Input");
        }
        if (address->mScope == kAudioObjectPropertyScopeOutput) {
            return copy_cf_string("Output");
        }
        return copy_cf_string("Main");
    }

    if (address->mSelector == kAudioObjectPropertyElementNumberName) {
        return copy_cf_string("1");
    }

    return copy_cf_string("Main");
}

KoeVirtualMicDriver &driver_from_ref(const void *driver_ref) {
    if (driver_ref == nullptr) {
        return KoeVirtualMicDriver::instance();
    }
    const auto *storage = reinterpret_cast<const DriverRefStorage *>(driver_ref);
    if (storage->driver != nullptr) {
        return *storage->driver;
    }
    return KoeVirtualMicDriver::instance();
}
} // namespace

KoeVirtualMicDriver &KoeVirtualMicDriver::instance() {
    static KoeVirtualMicDriver driver;
    return driver;
}

AudioServerPlugInDriverInterface *KoeVirtualMicDriver::driver_interface() {
    static bool initialized = false;
    if (!initialized) {
        kDriverStorage.driver = &KoeVirtualMicDriver::instance();
        kDriverInterface._reserved = nullptr;
        kDriverInterface.QueryInterface = [](void *in_driver, REFIID in_uuid, LPVOID *out_interface) {
            return driver_from_ref(in_driver).query_interface(in_driver, in_uuid, out_interface);
        };
        kDriverInterface.AddRef = [](void *in_driver) {
            return driver_from_ref(in_driver).add_ref();
        };
        kDriverInterface.Release = [](void *in_driver) {
            return driver_from_ref(in_driver).release();
        };
        kDriverInterface.Initialize = [](AudioServerPlugInDriverRef in_driver, AudioServerPlugInHostRef in_host) {
            return driver_from_ref(in_driver).initialize(in_host);
        };
        kDriverInterface.CreateDevice = [](AudioServerPlugInDriverRef in_driver, CFDictionaryRef in_description, const AudioServerPlugInClientInfo *in_client_info, AudioObjectID *out_device_object_id) {
            return driver_from_ref(in_driver).create_device(out_device_object_id);
        };
        kDriverInterface.DestroyDevice = [](AudioServerPlugInDriverRef in_driver, AudioObjectID in_device_object_id) {
            return driver_from_ref(in_driver).destroy_device(in_device_object_id);
        };
        kDriverInterface.AddDeviceClient = [](AudioServerPlugInDriverRef in_driver, AudioObjectID in_device_object_id, const AudioServerPlugInClientInfo *in_client_info) {
            return driver_from_ref(in_driver).add_device_client(in_device_object_id, in_client_info);
        };
        kDriverInterface.RemoveDeviceClient = [](AudioServerPlugInDriverRef in_driver, AudioObjectID in_device_object_id, const AudioServerPlugInClientInfo *in_client_info) {
            return driver_from_ref(in_driver).remove_device_client(in_device_object_id, in_client_info);
        };
        kDriverInterface.PerformDeviceConfigurationChange = [](AudioServerPlugInDriverRef in_driver, AudioObjectID in_device_object_id, UInt64 in_change_action, void *in_change_info) {
            return driver_from_ref(in_driver).perform_device_configuration_change(in_device_object_id, in_change_action, in_change_info);
        };
        kDriverInterface.AbortDeviceConfigurationChange = [](AudioServerPlugInDriverRef in_driver, AudioObjectID in_device_object_id, UInt64 in_change_action, void *in_change_info) {
            return driver_from_ref(in_driver).abort_device_configuration_change(in_device_object_id, in_change_action, in_change_info);
        };
        kDriverInterface.HasProperty = [](AudioServerPlugInDriverRef in_driver, AudioObjectID in_object_id, pid_t in_client_process_id, const AudioObjectPropertyAddress *in_address) {
            Boolean has = driver_from_ref(in_driver).has_property(in_object_id, in_address);
            if (!has && in_address != nullptr) {
                char c1 = (char)((in_address->mSelector >> 24) & 0xFF);
                char c2 = (char)((in_address->mSelector >> 16) & 0xFF);
                char c3 = (char)((in_address->mSelector >> 8) & 0xFF);
                char c4 = (char)(in_address->mSelector & 0xFF);
                os_log(OS_LOG_DEFAULT, "KoeVirtualMic: HasProperty returned false for %u ('%c%c%c%c') on object %d", in_address->mSelector, c1, c2, c3, c4, in_object_id);
            }
            return has;
        };
        kDriverInterface.IsPropertySettable = [](AudioServerPlugInDriverRef in_driver, AudioObjectID in_object_id, pid_t in_client_process_id, const AudioObjectPropertyAddress *in_address, Boolean *out_is_settable) {
            return driver_from_ref(in_driver).is_property_settable(in_object_id, in_address, out_is_settable);
        };
        kDriverInterface.GetPropertyDataSize = [](AudioServerPlugInDriverRef in_driver, AudioObjectID in_object_id, pid_t in_client_process_id, const AudioObjectPropertyAddress *in_address, UInt32 in_qualifier_data_size, const void *in_qualifier_data, UInt32 *out_data_size) {
            OSStatus status = driver_from_ref(in_driver).get_property_data_size(in_object_id, in_address, in_qualifier_data_size, in_qualifier_data, out_data_size);
            if (in_address != nullptr) {
                char c1 = (char)((in_address->mSelector >> 24) & 0xFF);
                char c2 = (char)((in_address->mSelector >> 16) & 0xFF);
                char c3 = (char)((in_address->mSelector >> 8) & 0xFF);
                char c4 = (char)(in_address->mSelector & 0xFF);
                os_log(OS_LOG_DEFAULT, "KoeVirtualMic: GetPropertyDataSize requested %u ('%c%c%c%c') on object %d, returning %d", in_address->mSelector, c1, c2, c3, c4, in_object_id, status);
            }
            return status;
        };
        kDriverInterface.GetPropertyData = [](AudioServerPlugInDriverRef in_driver, AudioObjectID in_object_id, pid_t in_client_process_id, const AudioObjectPropertyAddress *in_address, UInt32 in_qualifier_data_size, const void *in_qualifier_data, UInt32 in_data_size, UInt32 *out_data_size, void *out_data) {
            OSStatus status = driver_from_ref(in_driver).get_property_data(in_object_id, in_address, in_qualifier_data_size, in_qualifier_data, in_data_size, out_data_size, out_data);
            if (in_address != nullptr) {
                char c1 = (char)((in_address->mSelector >> 24) & 0xFF);
                char c2 = (char)((in_address->mSelector >> 16) & 0xFF);
                char c3 = (char)((in_address->mSelector >> 8) & 0xFF);
                char c4 = (char)(in_address->mSelector & 0xFF);
                os_log(OS_LOG_DEFAULT, "KoeVirtualMic: GetPropertyData requested %u ('%c%c%c%c') on object %d, returning %d", in_address->mSelector, c1, c2, c3, c4, in_object_id, status);
            }
            return status;
        };
        kDriverInterface.SetPropertyData = [](AudioServerPlugInDriverRef in_driver, AudioObjectID in_object_id, pid_t in_client_process_id, const AudioObjectPropertyAddress *in_address, UInt32 in_qualifier_data_size, const void *in_qualifier_data, UInt32 in_data_size, const void *in_data) {
            return driver_from_ref(in_driver).set_property_data(in_object_id, in_address, in_data_size, in_data);
        };
        kDriverInterface.StartIO = [](AudioServerPlugInDriverRef in_driver, AudioObjectID in_device_object_id, UInt32 in_client_id) {
            return driver_from_ref(in_driver).start_io(in_device_object_id, in_client_id);
        };
        kDriverInterface.StopIO = [](AudioServerPlugInDriverRef in_driver, AudioObjectID in_device_object_id, UInt32 in_client_id) {
            return driver_from_ref(in_driver).stop_io(in_device_object_id, in_client_id);
        };
        kDriverInterface.GetZeroTimeStamp = [](AudioServerPlugInDriverRef in_driver, AudioObjectID in_device_object_id, UInt32 in_client_id, Float64 *out_sample_time, UInt64 *out_host_time, UInt64 *out_seed) {
            return driver_from_ref(in_driver).get_zero_time_stamp(in_device_object_id, out_sample_time, out_host_time, out_seed);
        };
        kDriverInterface.WillDoIOOperation = [](AudioServerPlugInDriverRef in_driver, AudioObjectID in_device_object_id, UInt32 in_client_id, UInt32 in_operation_id, Boolean *out_will_do, Boolean *out_will_do_in_place) {
            return driver_from_ref(in_driver).will_do_io_operation(in_device_object_id, in_operation_id, out_will_do, out_will_do_in_place);
        };
        kDriverInterface.BeginIOOperation = [](AudioServerPlugInDriverRef in_driver, AudioObjectID in_device_object_id, UInt32 in_client_id, UInt32 in_operation_id, UInt32 in_io_buffer_frame_size, const AudioServerPlugInIOCycleInfo *in_io_cycle_info) {
            return driver_from_ref(in_driver).begin_io_operation(in_device_object_id, in_operation_id, in_io_buffer_frame_size, in_io_cycle_info);
        };
        kDriverInterface.DoIOOperation = [](AudioServerPlugInDriverRef in_driver, AudioObjectID in_device_object_id, AudioObjectID in_stream_object_id, UInt32 in_client_id, UInt32 in_operation_id, UInt32 in_io_buffer_frame_size, const AudioServerPlugInIOCycleInfo *in_io_cycle_info, void *io_main_buffer, void *io_secondary_buffer) {
            return driver_from_ref(in_driver).do_io_operation(in_device_object_id, in_stream_object_id, in_operation_id, in_io_buffer_frame_size, io_main_buffer, io_secondary_buffer);
        };
        kDriverInterface.EndIOOperation = [](AudioServerPlugInDriverRef in_driver, AudioObjectID in_device_object_id, UInt32 in_client_id, UInt32 in_operation_id, UInt32 in_io_buffer_frame_size, const AudioServerPlugInIOCycleInfo *in_io_cycle_info) {
            return driver_from_ref(in_driver).end_io_operation(in_device_object_id, in_operation_id, in_io_buffer_frame_size, in_io_cycle_info);
        };
        initialized = true;
    }
    return &kDriverInterface;
}

KoeVirtualMicDriver::KoeVirtualMicDriver()
    : ref_count_(1),
      host_(nullptr),
      zero_timestamp_seed_(1),
      sample_time_(0),
      active_io_clients_(0),
      last_render_state_(kRenderStateUnavailable),
      render_source_(static_cast<uint32_t>(kNominalSampleRate), kChannelCount) {}

HRESULT KoeVirtualMicDriver::query_interface(void *driver_ref, REFIID uuid, LPVOID *out_interface) {
    os_log(OS_LOG_DEFAULT, "KoeVirtualMic: query_interface called");
    if (out_interface == nullptr) {
        return E_POINTER;
    }
    *out_interface = nullptr;
    const CFUUIDRef driver_uuid = kAudioServerPlugInDriverInterfaceUUID;
    const CFUUIDRef unknown_uuid = IUnknownUUID;
    if (uuid_bytes_equal(uuid, driver_uuid) || uuid_bytes_equal(uuid, unknown_uuid)) {
        *out_interface = driver_ref != nullptr ? driver_ref : &kDriverStorage;
        add_ref();
        return S_OK;
    }
    return E_NOINTERFACE;
}

ULONG KoeVirtualMicDriver::add_ref() {
    return ++ref_count_;
}

ULONG KoeVirtualMicDriver::release() {
    const ULONG count = --ref_count_;
    if (count == 0) {
        ref_count_ = 1;
        return 0;
    }
    return count;
}

OSStatus KoeVirtualMicDriver::initialize(AudioServerPlugInHostRef host) {
    os_log(OS_LOG_DEFAULT, "KoeVirtualMic: initialize called with host=%p", host);
    host_ = host;
    os_log(OS_LOG_DEFAULT, "KoeVirtualMic: initialize returning success");
    return kAudioHardwareNoError;
}

OSStatus KoeVirtualMicDriver::create_device(AudioObjectID *out_device_object_id) const {
    os_log(OS_LOG_DEFAULT, "KoeVirtualMic: create_device called");
    if (out_device_object_id == nullptr) {
        os_log(OS_LOG_DEFAULT, "KoeVirtualMic: create_device failed - null pointer");
        return kAudioHardwareIllegalOperationError;
    }
    *out_device_object_id = kDeviceObjectID;
    os_log(OS_LOG_DEFAULT, "KoeVirtualMic: create_device returning device_id=%u", kDeviceObjectID);
    return kAudioHardwareNoError;
}

OSStatus KoeVirtualMicDriver::destroy_device(AudioObjectID device_object_id) const {
    return device_object_id == kDeviceObjectID ? kAudioHardwareNoError : kAudioHardwareBadObjectError;
}

OSStatus KoeVirtualMicDriver::add_device_client(AudioObjectID device_object_id, const AudioServerPlugInClientInfo *client_info) const {
    return device_object_id == kDeviceObjectID ? kAudioHardwareNoError : kAudioHardwareBadObjectError;
}

OSStatus KoeVirtualMicDriver::remove_device_client(AudioObjectID device_object_id, const AudioServerPlugInClientInfo *client_info) const {
    return device_object_id == kDeviceObjectID ? kAudioHardwareNoError : kAudioHardwareBadObjectError;
}

OSStatus KoeVirtualMicDriver::perform_device_configuration_change(AudioObjectID device_object_id, UInt64 change_action, void *change_info) const {
    if (device_object_id != kDeviceObjectID) {
        return kAudioHardwareBadObjectError;
    }
    notify_device_configuration_changed();
    return kAudioHardwareNoError;
}

OSStatus KoeVirtualMicDriver::abort_device_configuration_change(AudioObjectID device_object_id, UInt64 change_action, void *change_info) const {
    return device_object_id == kDeviceObjectID ? kAudioHardwareNoError : kAudioHardwareBadObjectError;
}

Boolean KoeVirtualMicDriver::has_property(AudioObjectID object_id, const AudioObjectPropertyAddress *address) const {
    if (!is_known_object(object_id) || address == nullptr) {
        return false;
    }

    switch (object_id) {
        case kPluginObjectID:
            return addresses_match_selector(address, kAudioObjectPropertyBaseClass) ||
                addresses_match_selector(address, kAudioObjectPropertyClass) ||
                addresses_match_selector(address, kAudioObjectPropertyCustomPropertyInfoList) ||
                addresses_match_selector(address, kAudioObjectPropertyOwner) ||
                addresses_match_selector(address, kAudioObjectPropertyModelName) ||
                addresses_match_selector(address, kAudioObjectPropertyManufacturer) ||
                addresses_match_selector(address, kAudioObjectPropertyOwnedObjects) ||
                addresses_match_selector(address, kAudioObjectPropertyName) ||
                addresses_match_selector(address, kAudioPlugInPropertyDeviceList) ||
                addresses_match_selector(address, kAudioPlugInPropertyTranslateUIDToDevice) ||
                addresses_match_selector(address, kAudioPlugInPropertyResourceBundle);
        case kDeviceObjectID:
            return addresses_match_selector(address, kAudioObjectPropertyBaseClass) ||
                addresses_match_selector(address, kAudioObjectPropertyClass) ||
                addresses_match_selector(address, kAudioObjectPropertyCustomPropertyInfoList) ||
                addresses_match_selector(address, kAudioObjectPropertyOwner) ||
                addresses_match_selector(address, kAudioObjectPropertyName) ||
                addresses_match_selector(address, kAudioObjectPropertyModelName) ||
                addresses_match_selector(address, kAudioObjectPropertyElementName) ||
                addresses_match_selector(address, kAudioObjectPropertyElementCategoryName) ||
                addresses_match_selector(address, kAudioObjectPropertyElementNumberName) ||
                addresses_match_selector(address, kAudioObjectPropertyManufacturer) ||
                addresses_match_selector(address, kAudioDevicePropertyDeviceUID) ||
                addresses_match_selector(address, kAudioDevicePropertyModelUID) ||
                addresses_match_selector(address, kAudioDevicePropertyTransportType) ||
                addresses_match_selector(address, kAudioDevicePropertyClockDomain) ||
                addresses_match_selector(address, kAudioDevicePropertyRelatedDevices) ||
                addresses_match_selector(address, kAudioDevicePropertyIsHidden) ||
                addresses_match_selector(address, kAudioDevicePropertyPreferredChannelsForStereo) ||
                addresses_match_selector(address, kAudioObjectPropertyOwnedObjects) ||
                addresses_match_selector(address, kAudioDevicePropertyStreams) ||
                addresses_match_selector(address, kAudioDevicePropertyLatency) ||
                addresses_match_selector(address, kAudioDevicePropertySafetyOffset) ||
                addresses_match_selector(address, kAudioDevicePropertyDeviceHasChanged) ||
                addresses_match_selector(address, kAudioDevicePropertyStreamConfiguration) ||
                addresses_match_selector(address, kAudioDevicePropertyNominalSampleRate) ||
                addresses_match_selector(address, kAudioDevicePropertyAvailableNominalSampleRates) ||
                addresses_match_selector(address, kAudioDevicePropertyDeviceIsAlive) ||
                addresses_match_selector(address, kAudioDevicePropertyDeviceIsRunning) ||
                addresses_match_selector(address, kAudioDevicePropertyDeviceIsRunningSomewhere) ||
                addresses_match_selector(address, kAudioDevicePropertyDeviceCanBeDefaultDevice) ||
                addresses_match_selector(address, kAudioDevicePropertyDeviceCanBeDefaultSystemDevice) ||
                addresses_match_selector(address, kAudioDevicePropertyZeroTimeStampPeriod) ||
                addresses_match_selector(address, kAudioDevicePropertyClockIsStable) ||
                addresses_match_selector(address, kAudioDevicePropertyHogMode) ||
                addresses_match_selector(address, kAudioObjectPropertyControlList) ||
                addresses_match_selector(address, kPropertyListenerRemoved) ||
                addresses_match_selector(address, kPropertyTapList);
        case kStreamObjectID:
            if (!is_stream_scope(address)) {
                return false;
            }
            return addresses_match_selector(address, kAudioObjectPropertyBaseClass) ||
                addresses_match_selector(address, kAudioObjectPropertyClass) ||
                addresses_match_selector(address, kAudioObjectPropertyCustomPropertyInfoList) ||
                addresses_match_selector(address, kAudioObjectPropertyOwner) ||
                addresses_match_selector(address, kAudioObjectPropertyName) ||
                addresses_match_selector(address, kAudioObjectPropertyModelName) ||
                addresses_match_selector(address, kAudioObjectPropertyElementName) ||
                addresses_match_selector(address, kAudioObjectPropertyElementCategoryName) ||
                addresses_match_selector(address, kAudioObjectPropertyElementNumberName) ||
                addresses_match_selector(address, kAudioStreamPropertyIsActive) ||
                addresses_match_selector(address, kAudioStreamPropertyDirection) ||
                addresses_match_selector(address, kAudioStreamPropertyStartingChannel) ||
                addresses_match_selector(address, kAudioStreamPropertyLatency) ||
                addresses_match_selector(address, kAudioStreamPropertyTerminalType) ||
                addresses_match_selector(address, kAudioStreamPropertyVirtualFormat) ||
                addresses_match_selector(address, kAudioStreamPropertyAvailableVirtualFormats) ||
                addresses_match_selector(address, kAudioStreamPropertyPhysicalFormat) ||
                addresses_match_selector(address, kAudioStreamPropertyAvailablePhysicalFormats) ||
                addresses_match_selector(address, kAudioObjectPropertyOwnedObjects) ||
                addresses_match_selector(address, kPropertyTapList);
        default: {
            char selector_str[5] = {0};
            selector_str[0] = (char)((address->mSelector >> 24) & 0xFF);
            selector_str[1] = (char)((address->mSelector >> 16) & 0xFF);
            selector_str[2] = (char)((address->mSelector >> 8) & 0xFF);
            selector_str[3] = (char)(address->mSelector & 0xFF);
            os_log(OS_LOG_DEFAULT, "KoeVirtualMic: has_property returned false for: '%s' on object %d", selector_str, object_id);
            return false;
        }
    }
}

OSStatus KoeVirtualMicDriver::is_property_settable(AudioObjectID object_id, const AudioObjectPropertyAddress *address, Boolean *out_is_settable) const {
    if (out_is_settable == nullptr) {
        return kAudioHardwareIllegalOperationError;
    }
    if (!has_property(object_id, address)) {
        char selector_str[5] = {0};
        selector_str[0] = (char)((address->mSelector >> 24) & 0xFF);
        selector_str[1] = (char)((address->mSelector >> 16) & 0xFF);
        selector_str[2] = (char)((address->mSelector >> 8) & 0xFF);
        selector_str[3] = (char)(address->mSelector & 0xFF);
        os_log(OS_LOG_DEFAULT, "KoeVirtualMic: unknown property requested: '%s' for object %d", selector_str, object_id);
        return kAudioHardwareUnknownPropertyError;
    }
    *out_is_settable = object_id == kDeviceObjectID && addresses_match_selector(address, kAudioDevicePropertyNominalSampleRate);
    return kAudioHardwareNoError;
}

OSStatus KoeVirtualMicDriver::get_property_data_size(AudioObjectID object_id, const AudioObjectPropertyAddress *address, UInt32 qualifier_data_size, const void *qualifier_data, UInt32 *out_data_size) const {
    if (out_data_size == nullptr) {
        return kAudioHardwareIllegalOperationError;
    }
    if (!has_property(object_id, address)) {
        char selector_str[5] = {0};
        selector_str[0] = (char)((address->mSelector >> 24) & 0xFF);
        selector_str[1] = (char)((address->mSelector >> 16) & 0xFF);
        selector_str[2] = (char)((address->mSelector >> 8) & 0xFF);
        selector_str[3] = (char)(address->mSelector & 0xFF);
        os_log(OS_LOG_DEFAULT, "KoeVirtualMic: unknown property requested: '%s' for object %d", selector_str, object_id);
        return kAudioHardwareUnknownPropertyError;
    }

    switch (address->mSelector) {
        case kAudioPlugInPropertyDeviceList:
            *out_data_size = sizeof(AudioObjectID);
            break;
        case kAudioObjectPropertyCustomPropertyInfoList:
            *out_data_size = 0;
            break;
        case kAudioObjectPropertyOwnedObjects:
            if (object_id == kPluginObjectID || object_id == kDeviceObjectID) {
                *out_data_size = sizeof(AudioObjectID);
            } else {
                *out_data_size = 0;
            }
            break;
        case kAudioDevicePropertyStreams:
            *out_data_size = is_output_scope(address) ? 0 : sizeof(AudioObjectID);
            break;
        case kAudioObjectPropertyControlList:
        case kPropertyTapList:
            *out_data_size = 0;
            break;
        case kAudioObjectPropertyBaseClass:
        case kAudioObjectPropertyClass:
            *out_data_size = sizeof(AudioClassID);
            break;
        case kAudioObjectPropertyOwner:
            *out_data_size = sizeof(AudioObjectID);
            break;
        case kAudioPlugInPropertyTranslateUIDToDevice:
            if (qualifier_data_size != sizeof(CFStringRef) || qualifier_data == nullptr) {
                return kAudioHardwareIllegalOperationError;
            }
            *out_data_size = sizeof(AudioObjectID);
            break;
        case kAudioDevicePropertyAvailableNominalSampleRates:
            *out_data_size = sizeof(AudioValueRange);
            break;
        case kAudioStreamPropertyAvailableVirtualFormats:
        case kAudioStreamPropertyAvailablePhysicalFormats:
            *out_data_size = sizeof(AudioStreamRangedDescription);
            break;
        case kAudioDevicePropertyNominalSampleRate:
            *out_data_size = sizeof(Float64);
            break;
        case kAudioDevicePropertyStreamConfiguration:
            *out_data_size = single_buffer_list_size();
            break;
        case kAudioDevicePropertyZeroTimeStampPeriod:
        case kAudioDevicePropertyTransportType:
        case kAudioDevicePropertyClockDomain:
        case kAudioDevicePropertyLatency:
        case kAudioDevicePropertySafetyOffset:
        case kAudioDevicePropertyDeviceHasChanged:
        case kAudioStreamPropertyStartingChannel:
        case kPropertyListenerRemoved:
            *out_data_size = sizeof(UInt32);
            break;
        case kAudioDevicePropertyClockIsStable:
        case kAudioDevicePropertyDeviceIsAlive:
        case kAudioDevicePropertyDeviceIsRunning:
        case kAudioDevicePropertyDeviceIsRunningSomewhere:
        case kAudioDevicePropertyDeviceCanBeDefaultDevice:
        case kAudioDevicePropertyDeviceCanBeDefaultSystemDevice:
        case kAudioStreamPropertyDirection:
        case kAudioStreamPropertyTerminalType:
        case kAudioStreamPropertyIsActive:
            *out_data_size = sizeof(UInt32);
            break;
        case kAudioDevicePropertyHogMode:
            *out_data_size = sizeof(pid_t);
            break;
        case kAudioDevicePropertyPreferredChannelsForStereo:
            *out_data_size = 2 * sizeof(UInt32);
            break;
        case kAudioObjectPropertyName:
        case kAudioObjectPropertyModelName:
        case kAudioObjectPropertyElementName:
        case kAudioObjectPropertyElementCategoryName:
        case kAudioObjectPropertyElementNumberName:
        case kAudioObjectPropertyManufacturer:
        case kAudioDevicePropertyDeviceUID:
        case kAudioDevicePropertyModelUID:
        case kAudioPlugInPropertyResourceBundle:
            *out_data_size = sizeof(CFStringRef);
            break;
        case kAudioStreamPropertyVirtualFormat:
        case kAudioStreamPropertyPhysicalFormat:
            *out_data_size = sizeof(AudioStreamBasicDescription);
            break;
        case kAudioDevicePropertyRelatedDevices:
            *out_data_size = sizeof(AudioObjectID);
            break;
        case kAudioDevicePropertyIsHidden:
            *out_data_size = sizeof(UInt32);
            break;
        default: {
            char selector_str[5] = {0};
            selector_str[0] = (char)((address->mSelector >> 24) & 0xFF);
            selector_str[1] = (char)((address->mSelector >> 16) & 0xFF);
            selector_str[2] = (char)((address->mSelector >> 8) & 0xFF);
            selector_str[3] = (char)(address->mSelector & 0xFF);
            os_log(OS_LOG_DEFAULT, "KoeVirtualMic: get_property_data_size missing in switch: '%s' for object %d", selector_str, object_id);
            return kAudioHardwareUnknownPropertyError;
        }
    }
    return kAudioHardwareNoError;
}

OSStatus KoeVirtualMicDriver::get_property_data(AudioObjectID object_id, const AudioObjectPropertyAddress *address, UInt32 qualifier_data_size, const void *qualifier_data, UInt32 in_data_size, UInt32 *out_data_size, void *out_data) const {
    if (out_data_size == nullptr || out_data == nullptr) {
        return kAudioHardwareIllegalOperationError;
    }
    UInt32 required_size = 0;
    OSStatus status = get_property_data_size(object_id, address, qualifier_data_size, qualifier_data, &required_size);
    if (status != kAudioHardwareNoError) {
        return status;
    }
    if (in_data_size < required_size) {
        return kAudioHardwareBadPropertySizeError;
    }

    switch (address->mSelector) {
        case kAudioObjectPropertyBaseClass: {
            auto *class_id = reinterpret_cast<AudioClassID *>(out_data);
            *class_id = kAudioObjectClassID;
            break;
        }
        case kAudioObjectPropertyClass: {
            auto *class_id = reinterpret_cast<AudioClassID *>(out_data);
            if (object_id == kPluginObjectID) {
                *class_id = kAudioPlugInClassID;
            } else if (object_id == kDeviceObjectID) {
                *class_id = kAudioDeviceClassID;
            } else {
                *class_id = kAudioStreamClassID;
            }
            break;
        }
        case kAudioObjectPropertyOwner: {
            auto *owner = reinterpret_cast<AudioObjectID *>(out_data);
            if (object_id == kPluginObjectID) {
                *owner = kAudioObjectUnknown;
            } else if (object_id == kDeviceObjectID) {
                *owner = kPluginObjectID;
            } else {
                *owner = kDeviceObjectID;
            }
            break;
        }
        case kAudioPlugInPropertyDeviceList: {
            *reinterpret_cast<AudioObjectID *>(out_data) = kDeviceObjectID;
            break;
        }
        case kAudioObjectPropertyControlList:
        case kPropertyTapList:
        case kAudioObjectPropertyCustomPropertyInfoList: {
            break;
        }
        case kAudioDevicePropertyRelatedDevices: {
            *reinterpret_cast<AudioObjectID *>(out_data) = kDeviceObjectID;
            break;
        }
        case kAudioObjectPropertyOwnedObjects: {
            if (required_size > 0) {
                *reinterpret_cast<AudioObjectID *>(out_data) = (object_id == kPluginObjectID) ? kDeviceObjectID : kStreamObjectID;
            }
            break;
        }
        case kAudioDevicePropertyStreams: {
            if (required_size > 0) {
                *reinterpret_cast<AudioObjectID *>(out_data) = kStreamObjectID;
            }
            break;
        }
        case kAudioDevicePropertyNominalSampleRate: {
            *reinterpret_cast<Float64 *>(out_data) = kNominalSampleRate;
            break;
        }
        case kAudioDevicePropertyTransportType: {
            *reinterpret_cast<UInt32 *>(out_data) = kAudioDeviceTransportTypeVirtual;
            break;
        }
        case kAudioDevicePropertyClockDomain: {
            *reinterpret_cast<UInt32 *>(out_data) = 0;
            break;
        }
        case kAudioDevicePropertyAvailableNominalSampleRates: {
            auto *range = reinterpret_cast<AudioValueRange *>(out_data);
            range->mMinimum = kNominalSampleRate;
            range->mMaximum = kNominalSampleRate;
            break;
        }
        case kAudioDevicePropertyStreamConfiguration: {
            auto *buffer_list = reinterpret_cast<AudioBufferList *>(out_data);
            buffer_list->mNumberBuffers = 1;
            buffer_list->mBuffers[0].mNumberChannels = is_output_scope(address) ? 0 : kChannelCount;
            buffer_list->mBuffers[0].mDataByteSize = 0;
            buffer_list->mBuffers[0].mData = nullptr;
            break;
        }
        case kAudioDevicePropertyZeroTimeStampPeriod: {
            *reinterpret_cast<UInt32 *>(out_data) = kZeroTimeStampPeriod;
            break;
        }
        case kAudioDevicePropertyLatency:
        case kAudioDevicePropertySafetyOffset: {
            *reinterpret_cast<UInt32 *>(out_data) = 0;
            break;
        }
        case kAudioDevicePropertyDeviceHasChanged: {
            *reinterpret_cast<UInt32 *>(out_data) = 0;
            break;
        }
        case kPropertyListenerRemoved: {
            *reinterpret_cast<UInt32 *>(out_data) = 0;
            break;
        }
        case kAudioDevicePropertyClockIsStable:
        case kAudioDevicePropertyDeviceIsAlive:
        case kAudioDevicePropertyDeviceIsRunning:
        case kAudioDevicePropertyDeviceIsRunningSomewhere:
        case kAudioDevicePropertyDeviceCanBeDefaultDevice:
        case kAudioDevicePropertyDeviceCanBeDefaultSystemDevice: {
            UInt32 value = 1;
            if (address->mSelector == kAudioDevicePropertyDeviceCanBeDefaultSystemDevice) {
                value = 0;
            }
            if (address->mSelector == kAudioDevicePropertyDeviceIsRunning ||
                address->mSelector == kAudioDevicePropertyDeviceIsRunningSomewhere) {
                value = is_running() ? 1U : 0U;
            }
            *reinterpret_cast<UInt32 *>(out_data) = value;
            break;
        }
        case kAudioDevicePropertyHogMode: {
            *reinterpret_cast<pid_t *>(out_data) = -1;
            break;
        }
        case kAudioDevicePropertyIsHidden: {
            *reinterpret_cast<UInt32 *>(out_data) = 0;
            break;
        }
        case kAudioDevicePropertyPreferredChannelsForStereo: {
            auto *channels = reinterpret_cast<UInt32 *>(out_data);
            channels[0] = 1;
            channels[1] = 1;
            break;
        }
        case kAudioStreamPropertyDirection: {
            *reinterpret_cast<UInt32 *>(out_data) = 1;
            break;
        }
        case kAudioStreamPropertyIsActive: {
            *reinterpret_cast<UInt32 *>(out_data) = 1;
            break;
        }
        case kAudioStreamPropertyStartingChannel: {
            *reinterpret_cast<UInt32 *>(out_data) = 1;
            break;
        }
        case kAudioStreamPropertyTerminalType: {
            *reinterpret_cast<UInt32 *>(out_data) = kAudioStreamTerminalTypeMicrophone;
            break;
        }
        case kAudioStreamPropertyVirtualFormat: {
            *reinterpret_cast<AudioStreamBasicDescription *>(out_data) = make_mono_float_format();
            break;
        }
        case kAudioStreamPropertyPhysicalFormat: {
            *reinterpret_cast<AudioStreamBasicDescription *>(out_data) = make_mono_float_format();
            break;
        }
        case kAudioStreamPropertyAvailableVirtualFormats:
        case kAudioStreamPropertyAvailablePhysicalFormats: {
            auto *desc = reinterpret_cast<AudioStreamRangedDescription *>(out_data);
            memset(desc, 0, sizeof(AudioStreamRangedDescription));
            desc->mFormat = make_mono_float_format();
            desc->mSampleRateRange.mMinimum = kNominalSampleRate;
            desc->mSampleRateRange.mMaximum = kNominalSampleRate;
            break;
        }
        case kAudioPlugInPropertyTranslateUIDToDevice: {
            const CFStringRef uid = *reinterpret_cast<const CFStringRef *>(qualifier_data);
            auto *translated_device_id = reinterpret_cast<AudioObjectID *>(out_data);
            if (uid != nullptr && CFStringCompare(uid, CFSTR("koe.virtual.mic.device"), 0) == kCFCompareEqualTo) {
                *translated_device_id = kDeviceObjectID;
            } else {
                *translated_device_id = kAudioObjectUnknown;
            }
            break;
        }
        case kAudioObjectPropertyName: {
            const char *name = kDeviceName;
            if (object_id == kPluginObjectID) {
                name = kPluginName;
            } else if (object_id == kStreamObjectID) {
                name = kStreamName;
            }
            *reinterpret_cast<CFStringRef *>(out_data) = copy_cf_string(name);
            break;
        }
        case kAudioObjectPropertyModelName: {
            const char *name = kDeviceName;
            if (object_id == kPluginObjectID) {
                name = kPluginName;
            } else if (object_id == kStreamObjectID) {
                name = kStreamName;
            }
            *reinterpret_cast<CFStringRef *>(out_data) = copy_cf_string(name);
            break;
        }
        case kAudioObjectPropertyElementName:
        case kAudioObjectPropertyElementCategoryName:
        case kAudioObjectPropertyElementNumberName: {
            *reinterpret_cast<CFStringRef *>(out_data) = copy_element_name(address);
            break;
        }
        case kAudioObjectPropertyManufacturer: {
            *reinterpret_cast<CFStringRef *>(out_data) = copy_cf_string(kManufacturerName);
            break;
        }
        case kAudioDevicePropertyDeviceUID: {
            *reinterpret_cast<CFStringRef *>(out_data) = copy_cf_string(kDeviceUID);
            break;
        }
        case kAudioDevicePropertyModelUID: {
            *reinterpret_cast<CFStringRef *>(out_data) = copy_cf_string(kModelUID);
            break;
        }
        case kAudioPlugInPropertyResourceBundle: {
            *reinterpret_cast<CFStringRef *>(out_data) = copy_cf_string(kResourceBundleName);
            break;
        }
        default: {
            char selector_str[5] = {0};
            selector_str[0] = (char)((address->mSelector >> 24) & 0xFF);
            selector_str[1] = (char)((address->mSelector >> 16) & 0xFF);
            selector_str[2] = (char)((address->mSelector >> 8) & 0xFF);
            selector_str[3] = (char)(address->mSelector & 0xFF);
            os_log(OS_LOG_DEFAULT, "KoeVirtualMic: get_property_data missing in switch: '%s' for object %d", selector_str, object_id);
            return kAudioHardwareUnknownPropertyError;
        }
    }

    *out_data_size = required_size;
    return kAudioHardwareNoError;
}

OSStatus KoeVirtualMicDriver::set_property_data(AudioObjectID object_id, const AudioObjectPropertyAddress *address, UInt32 in_data_size, const void *in_data) const {
    if (address == nullptr || in_data == nullptr) {
        return kAudioHardwareIllegalOperationError;
    }
    if (object_id != kDeviceObjectID) {
        return kAudioHardwareBadObjectError;
    }
    if (address->mSelector == kAudioDevicePropertyNominalSampleRate) {
        if (in_data_size != sizeof(Float64)) {
            return kAudioHardwareBadPropertySizeError;
        }
        const Float64 requested_rate = *reinterpret_cast<const Float64 *>(in_data);
        return requested_rate == kNominalSampleRate ? kAudioHardwareNoError : kAudioHardwareIllegalOperationError;
    }
    return kAudioHardwareUnsupportedOperationError;
}

OSStatus KoeVirtualMicDriver::start_io(AudioObjectID device_object_id, UInt32 client_id) {
    if (device_object_id != kDeviceObjectID) {
        os_log(OS_LOG_DEFAULT, "KoeVirtualMic: StartIO rejected unknown device=%u client=%u", device_object_id, client_id);
        return kAudioHardwareBadObjectError;
    }
    const bool was_running = is_running();
    ++active_io_clients_;
    const UInt32 clients = io_client_count();
    os_log(OS_LOG_DEFAULT, "KoeVirtualMic: StartIO device=%u client=%u active_clients=%u", device_object_id, client_id, clients);
    if (!was_running && is_running()) {
        notify_io_state_changed();
    }
    return kAudioHardwareNoError;
}

OSStatus KoeVirtualMicDriver::stop_io(AudioObjectID device_object_id, UInt32 client_id) {
    if (device_object_id != kDeviceObjectID) {
        os_log(OS_LOG_DEFAULT, "KoeVirtualMic: StopIO rejected unknown device=%u client=%u", device_object_id, client_id);
        return kAudioHardwareBadObjectError;
    }
    const bool was_running = is_running();
    if (active_io_clients_ > 0) {
        --active_io_clients_;
    }
    const UInt32 clients = io_client_count();
    os_log(OS_LOG_DEFAULT, "KoeVirtualMic: StopIO device=%u client=%u active_clients=%u", device_object_id, client_id, clients);
    if (was_running && !is_running()) {
        notify_io_state_changed();
    }
    return kAudioHardwareNoError;
}

OSStatus KoeVirtualMicDriver::get_zero_time_stamp(AudioObjectID device_object_id, Float64 *out_sample_time, UInt64 *out_host_time, UInt64 *out_seed) const {
    if (device_object_id != kDeviceObjectID || out_sample_time == nullptr || out_host_time == nullptr || out_seed == nullptr) {
        return kAudioHardwareIllegalOperationError;
    }
    *out_sample_time = sample_time_.load();
    *out_host_time = mach_absolute_time();
    *out_seed = zero_timestamp_seed_.load();
    return kAudioHardwareNoError;
}

OSStatus KoeVirtualMicDriver::will_do_io_operation(AudioObjectID device_object_id, UInt32 operation_id, Boolean *out_will_do, Boolean *out_will_do_in_place) const {
    if (device_object_id != kDeviceObjectID || out_will_do == nullptr || out_will_do_in_place == nullptr) {
        os_log(OS_LOG_DEFAULT, "KoeVirtualMic: WillDoIOOperation invalid args device=%u op=%u", device_object_id, operation_id);
        return kAudioHardwareIllegalOperationError;
    }
    *out_will_do = (operation_id == kAudioServerPlugInIOOperationReadInput);
    *out_will_do_in_place = true;
    os_log(OS_LOG_DEFAULT, "KoeVirtualMic: WillDoIOOperation device=%u op=%u will_do=%{public}s in_place=%{public}s", device_object_id, operation_id, *out_will_do ? "true" : "false", *out_will_do_in_place ? "true" : "false");
    return kAudioHardwareNoError;
}

OSStatus KoeVirtualMicDriver::begin_io_operation(AudioObjectID device_object_id, UInt32 operation_id, UInt32 io_buffer_frame_size, const AudioServerPlugInIOCycleInfo *io_cycle_info) const {
    if (device_object_id != kDeviceObjectID) {
        os_log(OS_LOG_DEFAULT, "KoeVirtualMic: BeginIOOperation rejected unknown device=%u op=%u frames=%u", device_object_id, operation_id, io_buffer_frame_size);
        return kAudioHardwareBadObjectError;
    }
    os_log(OS_LOG_DEFAULT, "KoeVirtualMic: BeginIOOperation device=%u op=%u frames=%u", device_object_id, operation_id, io_buffer_frame_size);
    return kAudioHardwareNoError;
}

OSStatus KoeVirtualMicDriver::do_io_operation(AudioObjectID device_object_id, AudioObjectID stream_object_id, UInt32 operation_id, UInt32 io_buffer_frame_size, void *io_main_buffer, void *io_secondary_buffer) {
    if (device_object_id != kDeviceObjectID || stream_object_id != kStreamObjectID) {
        os_log(OS_LOG_DEFAULT, "KoeVirtualMic: DoIOOperation rejected device=%u stream=%u op=%u frames=%u", device_object_id, stream_object_id, operation_id, io_buffer_frame_size);
        return kAudioHardwareBadObjectError;
    }
    os_log(OS_LOG_DEFAULT, "KoeVirtualMic: DoIOOperation device=%u stream=%u op=%u frames=%u", device_object_id, stream_object_id, operation_id, io_buffer_frame_size);
    if (operation_id != kAudioServerPlugInIOOperationReadInput) {
        return kAudioHardwareNoError;
    }

    auto *buffer = reinterpret_cast<Float32 *>(io_main_buffer != nullptr ? io_main_buffer : io_secondary_buffer);
    if (buffer == nullptr) {
        return kAudioHardwareIllegalOperationError;
    }

    const KoeVirtualMicRenderResult result = render_source_.render(buffer, io_buffer_frame_size);
    const UInt32 render_state = !result.source_available
        ? kRenderStateUnavailable
        : (!result.format_matches ? kRenderStateFormatMismatch : (result.frames_produced == 0 ? kRenderStateSilence : kRenderStateFlowing));
    const unsigned long long unread_frames = result.write_index_frames >= result.read_index_frames
        ? static_cast<unsigned long long>(result.write_index_frames - result.read_index_frames)
        : 0ULL;
    
    switch (render_state) {
        case kRenderStateUnavailable:
            os_log(OS_LOG_DEFAULT, "KoeVirtualMic: shared buffer unavailable at %{public}s", render_source_.reader().file_path().c_str());
            break;
        case kRenderStateFormatMismatch:
            os_log(OS_LOG_DEFAULT, "KoeVirtualMic: shared buffer format mismatch at %{public}s", render_source_.reader().file_path().c_str());
            break;
        case kRenderStateSilence:
            os_log(
                OS_LOG_DEFAULT,
                "KoeVirtualMic: shared buffer readable but empty, zero-filling %u frames (write=%llu read=%llu unread=%llu)",
                io_buffer_frame_size,
                static_cast<unsigned long long>(result.write_index_frames),
                static_cast<unsigned long long>(result.read_index_frames),
                unread_frames
            );
            break;
        case kRenderStateFlowing:
            os_log(
                OS_LOG_DEFAULT,
                "KoeVirtualMic: shared buffer flowing, produced %zu/%u frames at timestamp %llu (write=%llu read=%llu unread=%llu)",
                result.frames_produced,
                io_buffer_frame_size,
                static_cast<unsigned long long>(result.timestamp_ns),
                static_cast<unsigned long long>(result.write_index_frames),
                static_cast<unsigned long long>(result.read_index_frames),
                unread_frames
            );
            break;
    }

    sample_time_.store(sample_time_.load() + static_cast<Float64>(io_buffer_frame_size));
    zero_timestamp_seed_.store(result.timestamp_ns == 0 ? zero_timestamp_seed_.load() : result.timestamp_ns);
    return kAudioHardwareNoError;
}

OSStatus KoeVirtualMicDriver::end_io_operation(AudioObjectID device_object_id, UInt32 operation_id, UInt32 io_buffer_frame_size, const AudioServerPlugInIOCycleInfo *io_cycle_info) const {
    return device_object_id == kDeviceObjectID ? kAudioHardwareNoError : kAudioHardwareBadObjectError;
}

bool KoeVirtualMicDriver::is_known_object(AudioObjectID object_id) const {
    return object_id == kPluginObjectID || object_id == kDeviceObjectID || object_id == kStreamObjectID;
}

UInt32 KoeVirtualMicDriver::io_client_count() const {
    return active_io_clients_.load();
}

bool KoeVirtualMicDriver::is_running() const {
    return io_client_count() > 0;
}

OSStatus KoeVirtualMicDriver::notify_properties_changed(AudioObjectID object_id, UInt32 number_addresses, const AudioObjectPropertyAddress *addresses) const {
    if (host_ == nullptr || number_addresses == 0 || addresses == nullptr || host_->PropertiesChanged == nullptr) {
        return kAudioHardwareNoError;
    }
    return host_->PropertiesChanged(host_, object_id, number_addresses, addresses);
}

void KoeVirtualMicDriver::notify_io_state_changed() const {
    const AudioObjectPropertyAddress addresses[] = {
        {kAudioDevicePropertyDeviceIsRunning, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain},
        {kAudioDevicePropertyDeviceIsRunningSomewhere, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain},
    };
    static_cast<void>(notify_properties_changed(kDeviceObjectID, 2, addresses));
}

void KoeVirtualMicDriver::notify_device_configuration_changed() const {
    const AudioObjectPropertyAddress addresses[] = {
        {kAudioDevicePropertyDeviceHasChanged, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain},
        {kAudioObjectPropertyOwnedObjects, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain},
        {kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput, kAudioObjectPropertyElementMain},
        {kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain},
        {kAudioDevicePropertyStreamConfiguration, kAudioObjectPropertyScopeInput, kAudioObjectPropertyElementMain},
    };
    static_cast<void>(notify_properties_changed(kDeviceObjectID, 5, addresses));
}

extern "C" void *AudioServerPlugIn_Create(CFAllocatorRef allocator, CFUUIDRef type_uuid) {
    os_log(OS_LOG_DEFAULT, "KoeVirtualMic: AudioServerPlugIn_Create called with type_uuid=%p", type_uuid);
    if (type_uuid == nullptr || !CFEqual(type_uuid, kAudioServerPlugInTypeUUID)) {
        os_log(OS_LOG_DEFAULT, "KoeVirtualMic: AudioServerPlugIn_Create failed UUID check");
        return nullptr;
    }

    // Initialize the driver interface
    KoeVirtualMicDriver::driver_interface();

    // Return the driver reference (which will be passed to all callbacks)
    // This is the reference that gets passed as the first arg to all driver methods
    void *result = static_cast<void*>(&kDriverStorage);
    os_log(OS_LOG_DEFAULT, "KoeVirtualMic: AudioServerPlugIn_Create returning driver_ref=%p", result);
    return result;
}
