#include "shared_buffer_protocol.h"
#include "Support/shared_buffer_reader.h"
#include "koe_virtual_mic_render_source.h"

// Audio Server Plug-in stub.
// The real plug-in is implemented in koe_virtual_mic_driver.mm.

namespace {
SharedBufferReader gSharedBufferReader;
KoeVirtualMicRenderSource gRenderSource(48000, 1);
}
