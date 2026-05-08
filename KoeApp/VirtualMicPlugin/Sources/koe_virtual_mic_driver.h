#ifndef KOE_VIRTUAL_MIC_DRIVER_H
#define KOE_VIRTUAL_MIC_DRIVER_H

#include <CoreAudio/AudioServerPlugIn.h>

#include <atomic>

#include "koe_virtual_mic_render_source.h"

class KoeVirtualMicDriver {
public:
    static KoeVirtualMicDriver &instance();
    static AudioServerPlugInDriverInterface *driver_interface();

    static constexpr AudioObjectID kPluginObjectID = kAudioObjectPlugInObject;
    static constexpr AudioObjectID kDeviceObjectID = 2;
    static constexpr AudioObjectID kStreamObjectID = 3;

    HRESULT query_interface(void *driver_ref, REFIID uuid, LPVOID *out_interface);
    ULONG add_ref();
    ULONG release();

    OSStatus initialize(AudioServerPlugInHostRef host);
    OSStatus create_device(AudioObjectID *out_device_object_id) const;
    OSStatus destroy_device(AudioObjectID device_object_id) const;
    OSStatus add_device_client(AudioObjectID device_object_id, const AudioServerPlugInClientInfo *client_info) const;
    OSStatus remove_device_client(AudioObjectID device_object_id, const AudioServerPlugInClientInfo *client_info) const;
    OSStatus perform_device_configuration_change(AudioObjectID device_object_id, UInt64 change_action, void *change_info) const;
    OSStatus abort_device_configuration_change(AudioObjectID device_object_id, UInt64 change_action, void *change_info) const;

    Boolean has_property(AudioObjectID object_id, const AudioObjectPropertyAddress *address) const;
    OSStatus is_property_settable(AudioObjectID object_id, const AudioObjectPropertyAddress *address, Boolean *out_is_settable) const;
    OSStatus get_property_data_size(AudioObjectID object_id, const AudioObjectPropertyAddress *address, UInt32 qualifier_data_size, const void *qualifier_data, UInt32 *out_data_size) const;
    OSStatus get_property_data(AudioObjectID object_id, const AudioObjectPropertyAddress *address, UInt32 qualifier_data_size, const void *qualifier_data, UInt32 in_data_size, UInt32 *out_data_size, void *out_data) const;
    OSStatus set_property_data(AudioObjectID object_id, const AudioObjectPropertyAddress *address, UInt32 in_data_size, const void *in_data) const;

    OSStatus start_io(AudioObjectID device_object_id, UInt32 client_id);
    OSStatus stop_io(AudioObjectID device_object_id, UInt32 client_id);
    OSStatus get_zero_time_stamp(AudioObjectID device_object_id, Float64 *out_sample_time, UInt64 *out_host_time, UInt64 *out_seed) const;
    OSStatus will_do_io_operation(AudioObjectID device_object_id, UInt32 operation_id, Boolean *out_will_do, Boolean *out_will_do_in_place) const;
    OSStatus begin_io_operation(AudioObjectID device_object_id, UInt32 operation_id, UInt32 io_buffer_frame_size, const AudioServerPlugInIOCycleInfo *io_cycle_info) const;
    OSStatus do_io_operation(AudioObjectID device_object_id, AudioObjectID stream_object_id, UInt32 operation_id, UInt32 io_buffer_frame_size, void *io_main_buffer, void *io_secondary_buffer);
    OSStatus end_io_operation(AudioObjectID device_object_id, UInt32 operation_id, UInt32 io_buffer_frame_size, const AudioServerPlugInIOCycleInfo *io_cycle_info) const;

private:
    KoeVirtualMicDriver();

    bool is_known_object(AudioObjectID object_id) const;
    UInt32 io_client_count() const;
    bool is_running() const;
    OSStatus notify_properties_changed(AudioObjectID object_id, UInt32 number_addresses, const AudioObjectPropertyAddress *addresses) const;
    void notify_io_state_changed() const;
    void notify_device_configuration_changed() const;

    std::atomic<ULONG> ref_count_;
    AudioServerPlugInHostRef host_;
    mutable std::atomic<UInt64> zero_timestamp_seed_;
    mutable std::atomic<Float64> sample_time_;
    std::atomic<UInt32> active_io_clients_;
    std::atomic<UInt32> last_render_state_;
    KoeVirtualMicRenderSource render_source_;
};

extern "C" void *AudioServerPlugIn_Create(CFAllocatorRef allocator, CFUUIDRef type_uuid);

#endif
