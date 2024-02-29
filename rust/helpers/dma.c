// SPDX-License-Identifier: GPL-2.0

#include <linux/scatterlist.h>

struct device;
struct page;

void rust_helper_sg_set_page(struct scatterlist *sg, struct page *page,
                             unsigned int len, unsigned int offset)
{
    return sg_set_page(sg, page, len, offset);
}
EXPORT_SYMBOL_GPL(rust_helper_sg_set_page);

void rust_helper_dma_unmap_sgtable(struct device *dev, struct sg_table *sgt,
                                  enum dma_data_direction dir, unsigned long attrs)
{
    return dma_unmap_sgtable(dev, sgt, dir, attrs);
}
EXPORT_SYMBOL_GPL(rust_helper_dma_unmap_sgtable);
