/*
 * Copyright (C) 2007 Ben Skeggs.
 * All Rights Reserved.
 *
 * Permission is hereby granted, free of charge, to any person obtaining
 * a copy of this software and associated documentation files (the
 * "Software"), to deal in the Software without restriction, including
 * without limitation the rights to use, copy, modify, merge, publish,
 * distribute, sublicense, and/or sell copies of the Software, and to
 * permit persons to whom the Software is furnished to do so, subject to
 * the following conditions:
 *
 * The above copyright notice and this permission notice (including the
 * next paragraph) shall be included in all copies or substantial
 * portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
 * EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
 * MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
 * IN NO EVENT SHALL THE COPYRIGHT OWNER(S) AND/OR ITS SUPPLIERS BE
 * LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
 * OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
 * WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 *
 */

#ifndef __NOUVEAU_DMA_H__
#define __NOUVEAU_DMA_H__

#include "nouveau_bo.h"
#include "nouveau_chan.h"

int nouveau_dma_wait(struct nouveau_channel *, int slots, int size);
void nv50_dma_push(struct nouveau_channel *, u64 addr, u32 length,
		   bool no_prefetch);

/*
 * There's a hw race condition where you can't jump to your PUT offset,
 * to avoid this we jump to offset + SKIPS and fill the difference with
 * NOPs.
 *
 * xf86-video-nv configures the DMA fetch size to 32 bytes, and uses
 * a SKIPS value of 8.  Lets assume that the race condition is to do
 * with writing into the fetch area, we configure a fetch size of 128
 * bytes so we need a larger SKIPS value.
 */
#define NOUVEAU_DMA_SKIPS (128 / 4)

/* Maximum push buffer size. */
#define NV50_DMA_PUSH_MAX_LENGTH 0x7fffff

/* Maximum IBs per ring. */
#define NV50_DMA_IB_MAX ((0x02000 / 8) - 1)

static __must_check inline int
RING_SPACE(struct nouveau_channel *chan, int size)
{
	int ret;

	ret = nouveau_dma_wait(chan, 1, size);
	if (ret)
		return ret;

	chan->dma.free -= size;
	return 0;
}

static inline void
FIRE_RING(struct nouveau_channel *chan)
{
	if (chan->dma.cur == chan->dma.put)
		return;
	chan->accel_done = true;

	nv50_dma_push(chan, chan->push.addr + (chan->dma.put << 2),
		      (chan->dma.cur - chan->dma.put) << 2, false);

	chan->dma.put = chan->dma.cur;
}

static inline void
WIND_RING(struct nouveau_channel *chan)
{
	chan->dma.cur = chan->dma.put;
}

#endif
