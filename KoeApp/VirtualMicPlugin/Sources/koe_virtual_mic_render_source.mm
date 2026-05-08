#include "koe_virtual_mic_render_source.h"

#include <algorithm>
#include <utility>

KoeVirtualMicRenderSource::KoeVirtualMicRenderSource(
    std::uint32_t expected_sample_rate,
    std::uint32_t expected_channel_count,
    std::string file_path)
    : reader_(std::move(file_path)),
      expected_sample_rate_(expected_sample_rate),
      expected_channel_count_(expected_channel_count) {}

const SharedBufferReader &KoeVirtualMicRenderSource::reader() const {
    return reader_;
}

KoeVirtualMicRenderResult KoeVirtualMicRenderSource::render(float *out_samples, std::size_t max_frames) const {
    KoeVirtualMicRenderResult result {};
    result.frames_produced = 0;
    result.frames_silence_filled = max_frames;
    result.timestamp_ns = 0;
    result.write_index_frames = 0;
    result.read_index_frames = 0;
    result.source_available = false;
    result.format_matches = false;

    if (out_samples == nullptr || max_frames == 0) {
        return result;
    }

    KoeSharedBufferHeader header {};
    if (!reader_.read_header(header)) {
        std::fill(out_samples, out_samples + max_frames, 0.0f);
        return result;
    }

    result.source_available = true;
    result.write_index_frames = header.write_index_frames;
    result.read_index_frames = header.read_index_frames;
    result.format_matches = validate_format(header);
    if (!result.format_matches) {
        std::fill(out_samples, out_samples + max_frames, 0.0f);
        result.timestamp_ns = header.last_timestamp_ns;
        return result;
    }

    result.frames_produced = reader_.consume_mono_frames(
        out_samples,
        max_frames,
        result.timestamp_ns,
        result.write_index_frames,
        result.read_index_frames);
    result.frames_silence_filled = max_frames > result.frames_produced ? max_frames - result.frames_produced : 0;
    return result;
}

bool KoeVirtualMicRenderSource::probe_format(KoeSharedBufferHeader &header) const {
    if (!reader_.read_header(header)) {
        return false;
    }
    return validate_format(header);
}

bool KoeVirtualMicRenderSource::validate_format(const KoeSharedBufferHeader &header) const {
    return header.magic == KOE_SHARED_BUFFER_MAGIC &&
        header.version == KOE_SHARED_BUFFER_VERSION &&
        header.sample_rate == expected_sample_rate_ &&
        header.channel_count == expected_channel_count_;
}
