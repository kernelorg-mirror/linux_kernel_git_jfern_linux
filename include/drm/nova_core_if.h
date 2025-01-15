#ifndef NOVA_CORE_IF_H
#define NOVA_CORE_IF_H

#define TURING_CHANNEL_GPFIFO_A                       0x0000c46f
#define AMPERE_CHANNEL_GPFIFO_A                       0x0000c56f
#define AMPERE_CHANNEL_GPFIFO_B                       0x0000c76f

static const u8 NVIF_MEM_VRAM = 0x01;
static const u8 NVIF_MEM_HOST = 0x02;
static const u8 NVIF_MEM_COMP = 0x04;
static const u8 NVIF_MEM_DISP = 0x08;
static const u8 NVIF_MEM_KIND = 0x10;
static const u8 NVIF_MEM_MAPPABLE = 0x20;
static const u8 NVIF_MEM_COHERENT = 0x40;
static const u8 NVIF_MEM_UNCACHED = 0x80;

#define NVIF_PAGE_SPARSE 0x01
#define NVIF_PAGE_VRAM 0x02
#define NVIF_PAGE_HOST 0x04
#define NVIF_PAGE_COMP 0x08

static const u8 NVIF_MEM_OBJ_VRAM = 1;
static const u8 NVIF_MEM_OBJ_DMA = 2;
static const u8 NVIF_MEM_OBJ_SGL = 3;

static const u8 NVIF_VMM_TYPE_UNMANAGED = 1;
static const u8 NVIF_VMM_TYPE_MANAGED = 2;
static const u8 NVIF_VMM_TYPE_RAW = 3;

static const u8 NVIF_VMM_GET_ADDR = 1;
static const u8 NVIF_VMM_GET_PTES = 2;
static const u8 NVIF_VMM_GET_LAZY = 3;

static const u8 NOVA_CORE_ENGINE_SW = 1;
static const u8 NOVA_CORE_ENGINE_GR = 2;
static const u8 NOVA_CORE_ENGINE_CE = 3;
static const u8 NOVA_CORE_ENGINE_NVDEC = 4;
static const u8 NOVA_CORE_ENGINE_NVENC = 5;

static const u8 NOVA_CORE_CHAN_INST_APER_INST = 0x1;
static const u8 NOVA_CORE_CHAN_INST_APER_VRAM = 0x2;
static const u8 NOVA_CORE_CHAN_INST_APER_HOST = 0x3;
static const u8 NOVA_CORE_CHAN_INST_APER_NCOH = 0x4;

struct nova_core_mem_info {
	u8 dmabits;
	u8 heap_nr;
	u8 type_nr;

	u8 kind_inv;
	u16 kind_nr;
	struct types {
		u8 mmu_type;
		u8 heap;
	} mmu_type[16];

	u8 kind[16];
};

struct nova_core_vmm_info {
	u64 start;
	u64 limit;
	u8 page_nr;

	struct pages {
		u8 shift;
		u8 page_flags;
	} page[8];
};

struct nova_core_info {
	uint64_t resource_addr[3];
	uint64_t resource_size[3];

	uint32_t chipset;

	uint16_t pci_vendor_id;
	uint16_t pci_device_id;

	uint64_t boot0;
	uint64_t ram_user;
	uint64_t gart_limit;
	//
	u32 vram_type;//?


	u32 fifo_class;
	u32 ce_class;
	// fifo
	u32 mthdbuf_size;
	u16 gr_units;
	u8 engine_nr;
	u8 runl_nr;
	// all channels are per-runlist

	struct engine {
		u8 eng_type;
	} engine[8];

	struct runl {
		u8 id;
		u16 chan_nr;
		u8 runq_nr;
		u8 engn_nr;

		struct {
			u8 engine;
			u8 inst;
		} engn[8];
	} runl[64];
};

struct auxiliary_device;

struct nova_core_gsp_client {
	void *gsp_client;
	void *gsp_device;
};

struct nova_core_mmu {
	void *arc;
	struct nova_core_mem_info info;
};

struct nova_core_vmm {
	void *arc;
	struct nova_core_vmm_info info;
};

struct nova_core_memory_obj {
	void *obj;
	u64 addr;
	u64 size;
	u8 obj_type;

	u8 mem_type;
	u8 page;

	u64 bar1_vma_addr;
	void *bar1_map_handle;
};

void nova_core_fill_info(struct auxiliary_device *auxdev, struct nova_core_info *info);

u64 nova_core_timer_time(struct auxiliary_device *auxdev);

int nova_core_alloc_gsp_client(struct auxiliary_device *auxdev,
			       struct nova_core_gsp_client *client);
void nova_core_free_gsp_client(struct nova_core_gsp_client *client);

int nova_core_alloc_mmu(struct auxiliary_device *auxdev,
			struct nova_core_mmu *mmu);
int nova_core_free_mmu(struct nova_core_mmu *mmu);

int nova_core_alloc_vmm(struct auxiliary_device *auxdev,
			struct nova_core_gsp_client *client,
			struct nova_core_mmu *mmu,
			u8 vmm_type,
			struct nova_core_vmm *vmm);
int nova_core_free_vmm(struct nova_core_vmm *vmm);

int nova_core_alloc_mem(struct auxiliary_device *auxdev,
			struct nova_core_mmu *mmu,
			const char *name,
			u8 mmu_type, bool contig,
			dma_addr_t *dma,
			u8 page, u64 size, struct nova_core_memory_obj *obj);
int nova_core_free_mem(struct nova_core_memory_obj *obj);

int nova_core_mem_bar1_map(struct auxiliary_device *auxdev,
			   struct nova_core_memory_obj *obj,
			   u32 kind);

int nova_core_mem_bar1_unmap(struct auxiliary_device *auxdev,
			     struct nova_core_memory_obj *obj);


struct nova_core_map_args {
	u64 addr;
	u64 size;
	u64 offset;
	u8 ro;
	u8 kind;
	u8 private;
	u8 vol;
};

int nova_core_vmm_map(struct auxiliary_device *auxdev,
		      struct nova_core_vmm *vmm,
		      const struct nova_core_map_args *args,
		      struct nova_core_memory_obj *obj);

//VMM accessors
int nova_core_vmm_get(struct nova_core_vmm *vmm, u8 get_type,
		      bool sparse, u8 page, u8 align, u64 size, u64 *addr);
int nova_core_vmm_put(struct nova_core_vmm *vmm, u64 addr);

//int nova_core_vmm_map(struct nova_core_vmm *vmm, u64 addr, u64 size, u64 offset);
int nova_core_vmm_unmap(struct nova_core_vmm *vmm, u64 addr);

int nova_core_vmm_raw_get(struct nova_core_vmm *vmm, u8 shift, u64 addr, u64 size);
int nova_core_vmm_raw_put(struct nova_core_vmm *vmm, u8 shift, u64 addr, u64 size);
int nova_core_vmm_raw_map(struct nova_core_vmm *vmm, u8 shift, u64 addr, u64 size, u64 offset);
int nova_core_vmm_raw_unmap(struct nova_core_vmm *vmm, u8 shift, u64 addr, u64 size, bool sparse);


//FIFOy/channely stuff?

struct nova_core_chan_info {
	u32 doorbell_token;
	u16 id;
};
	
struct nova_core_chan {
	void *arc;
	struct nova_core_chan_info info;
};

int nova_core_alloc_chan(struct auxiliary_device *auxdev,
			 struct nova_core_gsp_client *client,
			 u8 runl, bool chan_priv,
			 u64 offset, u64 length,
			 struct nova_core_vmm *vmm,
			 struct nova_core_memory_obj *userd,
			 const char *name,
			 struct nova_core_chan *chan);

int nova_core_chan_register_nonstall(struct auxiliary_device *auxdev,
				     struct nova_core_chan *chan,
				     int (*cb)(void *data), void *data);

int nova_core_free_chan(struct nova_core_chan *chan);

struct nova_core_chan_obj {
	void *arc;
	u32 handle;
	u32 class;
	u8 engine_type;
	u8 engine_inst;
};

int nova_core_chan_alloc_object(struct auxiliary_device *auxdev,
				struct nova_core_chan *chan,
				struct nova_core_chan_obj *cobj);

int nova_core_chan_free_object(struct auxiliary_device *auxdev,
			       struct nova_core_chan *chan,
			       struct nova_core_chan_obj *cobj);

struct nova_core_user_info {
	void *ptr;
	u32 size;
};

int nova_core_map_user(struct auxiliary_device *auxdev,
		       struct nova_core_user_info *user);

void nova_core_unmap_user(struct auxiliary_device *auxdev,
			  struct nova_core_user_info *user);

#endif
