// crates/audiolib-windows/src/asio_shim.cpp
// Adds read-only buffer/channel queries and the driver's panel to CPAL's loaded SDK driver.
#include "asio.h"
#include <cstring>
extern "C" long audiolib_asio_buffer_range(long *min, long *max, long *preferred, long *granularity) {
    return static_cast<long>(ASIOGetBufferSize(min, max, preferred, granularity));
}
extern "C" long audiolib_asio_channel_name(long channel, char *name) {
    ASIOChannelInfo info{};
    info.channel = channel;
    info.isInput = ASIOFalse;
    const auto error = ASIOGetChannelInfo(&info);
    if (error == ASE_OK) { std::memcpy(name, info.name, 32); name[31] = 0; }
    return static_cast<long>(error);
}
extern "C" long audiolib_asio_control_panel() { return static_cast<long>(ASIOControlPanel()); }
