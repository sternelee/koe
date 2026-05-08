#ifndef KOE_VIRTUAL_MIC_RENDER_SOURCE_H
#define KOE_VIRTUAL_MIC_RENDER_SOURCE_H

#include <cstddef>
#include <cstdint>
#include <string>

#include "Support/shared_buffer_reader.h"

struct KoeVirtualMicRenderResult {
    std::size_t frames_produced;
    std::size_t frames_silence_filled;
    std::uint64_t timestamp_ns;
    std::uint64_t write_index_frames;
    std::uint64_t read_index_frames;
    bool source_available;
    bool format_matches;
};

class KoeVirtualMicRenderSource {
public:
    KoeVirtualMicRenderSource(
        std::uint32_t expected_sample_rate,
        std::uint32_t expected_channel_count,
        std::string file_path = KOE_SHARED_BUFFER_FILE_PATH);

    const SharedBufferReader &reader() const;
    KoeVirtualMicRenderResult render(float *out_samples, std::size_t max_frames) const;
    bool probe_format(KoeSharedBufferHeader &header) const;

private:
    bool validate_format(const KoeSharedBufferHeader &header) const;

    SharedBufferReader reader_;
    std::uint32_t expected_sample_rate_;
    std::uint32_t expected_channel_count_;
};

#endif
