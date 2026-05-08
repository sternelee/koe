#include "shared_buffer_reader.h"

#include <algorithm>
#include <cstring>
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#include <utility>

namespace {
constexpr std::size_t header_size_bytes() {
    return (6 * sizeof(std::uint32_t)) + (3 * sizeof(std::uint64_t));
}

std::uint32_t read_u32_le(const unsigned char *data, std::size_t &cursor) {
    const std::uint32_t value = static_cast<std::uint32_t>(data[cursor]) |
        (static_cast<std::uint32_t>(data[cursor + 1]) << 8U) |
        (static_cast<std::uint32_t>(data[cursor + 2]) << 16U) |
        (static_cast<std::uint32_t>(data[cursor + 3]) << 24U);
    cursor += 4;
    return value;
}

std::uint64_t read_u64_le(const unsigned char *data, std::size_t &cursor) {
    std::uint64_t value = 0;
    for (std::size_t index = 0; index < 8; ++index) {
        value |= static_cast<std::uint64_t>(data[cursor + index]) << (index * 8U);
    }
    cursor += 8;
    return value;
}

} // namespace

SharedBufferReader::SharedBufferReader(std::string file_path)
    : file_path_(std::move(file_path)) {}

SharedBufferReader::~SharedBufferReader() {
    unmap();
}

const std::string &SharedBufferReader::file_path() const {
    return file_path_;
}

bool SharedBufferReader::ensure_mapped() const {
    if (has_mmap_) {
        struct stat st;
        if (stat(file_path_.c_str(), &st) == 0 &&
            static_cast<std::size_t>(st.st_size) == mmap_size_) {
            return true;
        }
        unmap();
    }

    int fd = open(file_path_.c_str(), O_RDONLY);
    if (fd < 0) {
        return false;
    }

    struct stat st;
    if (fstat(fd, &st) != 0) {
        close(fd);
        return false;
    }

    void *addr = mmap(nullptr, st.st_size, PROT_READ, MAP_SHARED, fd, 0);
    close(fd);

    if (addr == MAP_FAILED) {
        return false;
    }

    mmap_addr_ = addr;
    mmap_size_ = static_cast<std::size_t>(st.st_size);
    has_mmap_ = true;
    return true;
}

void SharedBufferReader::unmap() const {
    if (mmap_addr_ != nullptr && mmap_addr_ != MAP_FAILED) {
        munmap(mmap_addr_, mmap_size_);
    }
    mmap_addr_ = nullptr;
    mmap_size_ = 0;
    has_mmap_ = false;
}

bool SharedBufferReader::read_header(KoeSharedBufferHeader &header) const {
    if (!ensure_mapped()) {
        return false;
    }

    const unsigned char *data = static_cast<const unsigned char *>(mmap_addr_);
    std::size_t cursor = 0;
    header.magic = read_u32_le(data, cursor);
    header.version = read_u32_le(data, cursor);
    header.channel_count = read_u32_le(data, cursor);
    header.sample_rate = read_u32_le(data, cursor);
    header.capacity_frames = read_u32_le(data, cursor);
    header.sequence = read_u32_le(data, cursor);
    header.write_index_frames = read_u64_le(data, cursor);
    header.read_index_frames = read_u64_le(data, cursor);
    header.last_timestamp_ns = read_u64_le(data, cursor);

    return header.magic == KOE_SHARED_BUFFER_MAGIC && header.version == KOE_SHARED_BUFFER_VERSION;
}

std::size_t SharedBufferReader::consume_mono_frames(
    float *out_samples,
    std::size_t max_frames,
    std::uint64_t &timestamp_ns,
    std::uint64_t &write_index_frames,
    std::uint64_t &read_index_frames) const {
    if (out_samples == nullptr) {
        timestamp_ns = 0;
        write_index_frames = 0;
        read_index_frames = 0;
        return 0;
    }

    if (!ensure_mapped()) {
        timestamp_ns = 0;
        write_index_frames = 0;
        read_index_frames = 0;
        std::fill(out_samples, out_samples + max_frames, 0.0f);
        return 0;
    }

    const unsigned char *data = static_cast<const unsigned char *>(mmap_addr_);

    // Seqlock at offset 20 (was "reserved"). Odd = writer is writing.
    constexpr std::size_t kSequenceOffset = 20;
    const std::uint32_t seq_before =
        *reinterpret_cast<const volatile std::uint32_t *>(data + kSequenceOffset);
    if (seq_before & 1) {
        timestamp_ns = 0;
        write_index_frames = 0;
        read_index_frames = 0;
        std::fill(out_samples, out_samples + max_frames, 0.0f);
        return 0;
    }

    // Parse header directly from mapped memory.
    std::size_t cursor = 0;
    std::uint32_t magic = read_u32_le(data, cursor);
    std::uint32_t version = read_u32_le(data, cursor);
    std::uint32_t channel_count = read_u32_le(data, cursor);
    std::uint32_t sample_rate = read_u32_le(data, cursor);
    std::uint32_t capacity_frames = read_u32_le(data, cursor);
    /* sequence */ read_u32_le(data, cursor);
    std::uint64_t write_index = read_u64_le(data, cursor);
    std::uint64_t header_read_index = read_u64_le(data, cursor);
    std::uint64_t last_timestamp = read_u64_le(data, cursor);

    write_index_frames = write_index;

    if (magic != KOE_SHARED_BUFFER_MAGIC || version != KOE_SHARED_BUFFER_VERSION) {
        timestamp_ns = last_timestamp;
        read_index_frames = header_read_index;
        std::fill(out_samples, out_samples + max_frames, 0.0f);
        return 0;
    }

    const std::size_t channels = std::max<std::size_t>(channel_count, 1);
    const std::size_t sample_count = static_cast<std::size_t>(capacity_frames) * channels;

    // Validate mmap covers the expected data region.
    const std::size_t expected_size = header_size_bytes() + sample_count * sizeof(float);
    if (mmap_size_ < expected_size) {
        timestamp_ns = last_timestamp;
        read_index_frames = header_read_index;
        std::fill(out_samples, out_samples + max_frames, 0.0f);
        return 0;
    }

    const float *samples = reinterpret_cast<const float *>(data + header_size_bytes());
    const std::uint64_t earliest_frame = write_index > capacity_frames
        ? write_index - capacity_frames
        : 0;

    std::uint64_t local_read_index = 0;
    std::size_t frames_to_copy = 0;

    {
        std::lock_guard<std::mutex> lock(read_state_mutex_);
        if (!has_local_read_index_) {
            const std::uint64_t initial_backfill = std::min<std::uint64_t>(write_index, max_frames);
            local_read_index_frames_ = write_index - initial_backfill;
            has_local_read_index_ = true;
        }
        if (local_read_index_frames_ < earliest_frame) {
            local_read_index_frames_ = earliest_frame;
        }
        if (local_read_index_frames_ > write_index) {
            local_read_index_frames_ = write_index;
        }
        local_read_index = local_read_index_frames_;

        const std::size_t available_frames = std::min<std::uint64_t>(
            capacity_frames,
            write_index > local_read_index
                ? write_index - local_read_index
                : 0);
        frames_to_copy = std::min(max_frames, available_frames);
        const std::size_t start_frame = capacity_frames == 0
            ? 0
            : static_cast<std::size_t>(local_read_index % capacity_frames);
        read_index_frames = local_read_index;

        for (std::size_t frame = 0; frame < frames_to_copy; ++frame) {
            const std::size_t source_frame = capacity_frames == 0
                ? frame
                : (start_frame + frame) % capacity_frames;
            out_samples[frame] = samples[source_frame * channels];
        }

        local_read_index_frames_ = local_read_index + frames_to_copy;
    }

    // Verify seqlock is unchanged after reading samples.
    const std::uint32_t seq_after =
        *reinterpret_cast<const volatile std::uint32_t *>(data + kSequenceOffset);
    if (seq_after != seq_before) {
        timestamp_ns = 0;
        write_index_frames = 0;
        read_index_frames = 0;
        std::fill(out_samples, out_samples + max_frames, 0.0f);
        return 0;
    }

    for (std::size_t frame = frames_to_copy; frame < max_frames; ++frame) {
        out_samples[frame] = 0.0f;
    }

    timestamp_ns = last_timestamp;
    return frames_to_copy;
}

std::vector<float> SharedBufferReader::read_all_samples(KoeSharedBufferHeader &header) const {
    if (!read_header(header)) {
        return {};
    }

    if (!ensure_mapped()) {
        return {};
    }

    const std::size_t channels = std::max<std::size_t>(header.channel_count, 1);
    const std::size_t sample_count = static_cast<std::size_t>(header.capacity_frames) * channels;
    const std::size_t expected_size = header_size_bytes() + sample_count * sizeof(float);
    if (mmap_size_ < expected_size) {
        return {};
    }

    const float *samples = reinterpret_cast<const float *>(
        static_cast<const unsigned char *>(mmap_addr_) + header_size_bytes());
    return std::vector<float>(samples, samples + sample_count);
}
