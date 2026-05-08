#ifndef KOE_VIRTUAL_MIC_SHARED_BUFFER_READER_H
#define KOE_VIRTUAL_MIC_SHARED_BUFFER_READER_H

#include <cstddef>
#include <cstdint>
#include <mutex>
#include <string>
#include <vector>

#include "../shared_buffer_protocol.h"

class SharedBufferReader {
public:
    explicit SharedBufferReader(std::string file_path = KOE_SHARED_BUFFER_FILE_PATH);
    ~SharedBufferReader();

    const std::string &file_path() const;
    bool read_header(KoeSharedBufferHeader &header) const;
    std::size_t consume_mono_frames(
        float *out_samples,
        std::size_t max_frames,
        std::uint64_t &timestamp_ns,
        std::uint64_t &write_index_frames,
        std::uint64_t &read_index_frames) const;
    std::vector<float> read_all_samples(KoeSharedBufferHeader &header) const;

private:
    std::string file_path_;
    mutable std::mutex read_state_mutex_;
    mutable std::uint64_t local_read_index_frames_ = 0;
    mutable bool has_local_read_index_ = false;

    // mmap state
    mutable void *mmap_addr_ = nullptr;
    mutable std::size_t mmap_size_ = 0;
    mutable bool has_mmap_ = false;

    bool ensure_mapped() const;
    void unmap() const;
};

#endif
