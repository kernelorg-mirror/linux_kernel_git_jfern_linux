#define _GNU_SOURCE         /* See feature_test_macros(7) */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <unistd.h>
#include <sys/mman.h>
#include <string.h>

#define ALIGNMENT (2 * 1024 * 1024)
#define BLOCK_SIZE (10 * 1024 * 1024)
#define UNMAP_OFFSET (1 * 1024 * 1024)
#define UNMAP_SIZE (4 * 1024 * 1024)
#define REMAP_SIZE (5 * 1024 * 1024)

int main() {
	printf("new\n");
	// Allocate memory
	size_t alloc_size = BLOCK_SIZE + ALIGNMENT;
	void *raw_ptr = malloc(alloc_size);
	if (raw_ptr == NULL) {
		perror("malloc failed");
		return 1;
	}
	memset(raw_ptr, 0xff, alloc_size);
	printf("Allocated raw memory: %p\n", raw_ptr);

	// Align the memory
	uintptr_t aligned_addr = (((uintptr_t)raw_ptr + ALIGNMENT - 1) / ALIGNMENT) * ALIGNMENT;
	void *aligned_ptr = (void *)aligned_addr;
	printf("Aligned memory: %p\n", aligned_ptr);

	// Unmap memory
	// 0-1MB : alloc'd
	// 1-5MB : hole
	if (munmap(aligned_ptr + UNMAP_OFFSET, UNMAP_SIZE) == -1) {
		perror("munmap failed");
		free(raw_ptr);
		return 1;
	}
	printf("Unmapped %zu bytes starting at %p\n", (size_t)UNMAP_SIZE, aligned_ptr + UNMAP_OFFSET);

	// Move the second half of the block using mremap
	void *source_ptr = aligned_ptr + UNMAP_SIZE + UNMAP_OFFSET;
	void *remapped_ptr = aligned_ptr + UNMAP_OFFSET;
	printf("Moving %zu bytes from %p to %p\n", (size_t)REMAP_SIZE, source_ptr, remapped_ptr);
	void *moved_ptr = mremap(source_ptr, REMAP_SIZE, REMAP_SIZE, MREMAP_FIXED|MREMAP_MAYMOVE, remapped_ptr);

	if (moved_ptr == MAP_FAILED) {
		perror("mremap (move) failed");
		free(raw_ptr);
		return 1;
	}

	printf("Moved %zu bytes from %p to %p using mremap\n", (size_t)REMAP_SIZE, source_ptr, moved_ptr);

	// Clean up
	free(raw_ptr);

	return 0;
}

