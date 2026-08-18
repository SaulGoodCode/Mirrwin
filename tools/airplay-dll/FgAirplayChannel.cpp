#include "FgAirplayChannel.h"
#include "BridgeTap.h"
#include <windows.h>

// Overlay replacing upstream airplay2dll/FgAirplayChannel.cpp. Reference
// counting matches upstream; the FFmpeg decode path is gone (see the header).

FgAirplayChannel::FgAirplayChannel(IAirServerCallback* pCallback)
: m_nRef(1)
, m_pCallback(pCallback)
, m_fScaleRatio(1.0f)
{
}

FgAirplayChannel::~FgAirplayChannel()
{
	m_pCallback = NULL;
}

long FgAirplayChannel::addRef()
{
	InterlockedIncrement(&m_nRef);
	return (m_nRef > 1 ? m_nRef : 1);
}

long FgAirplayChannel::release()
{
	LONG lRef = InterlockedDecrement(&m_nRef);
	if (0 == lRef)
	{
		delete this;
		return 0;
	}
	return (m_nRef > 1 ? m_nRef : 1);
}

float FgAirplayChannel::setScale(float fRatio)
{
	m_fScaleRatio = fRatio;
	return m_fScaleRatio;
}

int FgAirplayChannel::decodeH264Data(SFgH264Data* data, const char* remoteName, const char* remoteDeviceId)
{
	if (!data || !data->data || data->size <= 0)
	{
		return 0;
	}
	bridge_on_h264(data->data, data->size, data->is_key);
	return 0;
}
