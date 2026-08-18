#pragma once
#include "Airplay2Head.h"

// Overlay replacing upstream airplay2dll/FgAirplayChannel.h.
//
// Upstream decodes H.264 to YUV with FFmpeg here and hands frames to
// IAirServerCallback::outputVideo. This host decodes H.264 itself (WebCodecs in
// the webview), so the channel forwards the elementary stream untouched and the
// avcodec/swscale/avutil dependency disappears with it.
//
// SFgH264Data keeps upstream's layout because FgAirplayServer.cpp fills it.

typedef struct SFgH264Data {
	int pts;
	int size;
	int is_key;
	int width;
	int height;
	unsigned char* data;
} SFgH264Data;

class FgAirplayChannel
{
public:
	explicit FgAirplayChannel(IAirServerCallback* pCallback);
	~FgAirplayChannel();

public:
	long addRef();
	long release();

	float setScale(float fRatio);
	int decodeH264Data(SFgH264Data* data, const char* remoteName, const char* remoteDeviceId);

protected:
	long m_nRef;
	IAirServerCallback* m_pCallback;
	float m_fScaleRatio;
};
