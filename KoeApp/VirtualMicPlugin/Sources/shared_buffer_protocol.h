#ifndef KOE_VIRTUAL_MIC_SHARED_BUFFER_H
#define KOE_VIRTUAL_MIC_SHARED_BUFFER_H

#include <stdint.h>

#define KOE_SHARED_BUFFER_MAGIC 0x4B4F4556u   // "KOEV" in little-endian
#define KOE_SHARED_BUFFER_VERSION 1u
#define KOE_SHARED_BUFFER_NAME "koe_virtual_mic_output"
#define KOE_SHARED_BUFFER_FILE_PATH "/tmp/koe/virtual_mic_output.bin"

#ifdef __cplusplus
extern "C" {
#endif

typedef struct KoeSharedBufferHeader {
    uint32_t magic;
    uint32_t version;
    uint32_t channel_count;
    uint32_t sample_rate;
    uint32_t capacity_frames;
    uint32_t sequence;       // seqlock: even = consistent, odd = writer active
    uint64_t write_index_frames;
    uint64_t read_index_frames;
    uint64_t last_timestamp_ns;
} KoeSharedBufferHeader;

#ifdef __cplusplus
}
#endif

#endif
