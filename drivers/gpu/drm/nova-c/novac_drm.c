
#include "novac_drv.h"


#include <drm/drm_ioctl.h>

#include <linux/auxiliary_bus.h>

#include "nouveau_abi16.h"
#include "nouveau_chan.h"
#include "nouveau_debugfs.h"
#include "nouveau_dma.h"
#include "nouveau_fence.h"
#include "nouveau_ioctl.h"
#include "nouveau_exec.h"
#include "nouveau_gem.h"
#include "nouveau_ttm.h"

#include "novac_helpers.h"
#include <drm/nova_core_if.h>
struct nvkm_device;

static inline bool
nouveau_cli_work_ready(struct dma_fence *fence)
{
	bool ret = true;

	spin_lock_irq(fence->lock);
	if (!dma_fence_is_signaled_locked(fence))
		ret = false;
	spin_unlock_irq(fence->lock);

	if (ret == true)
		dma_fence_put(fence);
	return ret;
}

static void
nouveau_cli_work(struct work_struct *w)
{
	struct nouveau_cli *cli = container_of(w, typeof(*cli), work);
	struct nouveau_cli_work *work, *wtmp;
	mutex_lock(&cli->lock);
	list_for_each_entry_safe(work, wtmp, &cli->worker, head) {
		if (!work->fence || nouveau_cli_work_ready(work->fence)) {
			list_del(&work->head);
			work->func(work);
		}
	}
	mutex_unlock(&cli->lock);
}

static void
nouveau_cli_work_fence(struct dma_fence *fence, struct dma_fence_cb *cb)
{
	struct nouveau_cli_work *work = container_of(cb, typeof(*work), cb);
	schedule_work(&work->cli->work);
}

void
nouveau_cli_work_queue(struct nouveau_cli *cli, struct dma_fence *fence,
		       struct nouveau_cli_work *work)
{
	work->fence = dma_fence_get(fence);
	work->cli = cli;
	mutex_lock(&cli->lock);
	list_add_tail(&work->head, &cli->worker);
	if (dma_fence_add_callback(fence, &work->cb, nouveau_cli_work_fence))
		nouveau_cli_work_fence(fence, &work->cb);
	mutex_unlock(&cli->lock);
}

static void
nouveau_cli_fini(struct nouveau_cli *cli)
{
	struct nouveau_uvmm *uvmm = nouveau_cli_uvmm_locked(cli);

	/* All our channels are dead now, which means all the fences they
	 * own are signalled, and all callback functions have been called.
	 *
	 * So, after flushing the workqueue, there should be nothing left.
	 */
	flush_work(&cli->work);
	WARN_ON(!list_empty(&cli->worker));

	if (cli->sched)
		nouveau_sched_destroy(&cli->sched);
	if (uvmm)
		nouveau_uvmm_fini(uvmm);
	nouveau_vmm_fini(&cli->vmm);
	nova_core_free_mmu(&cli->mmu);
	nova_core_free_gsp_client(&cli->gsp);
}

static int
nouveau_cli_init(struct nouveau_drm *drm, const char *sname,
		 struct nouveau_cli *cli)
{
	int ret;

	snprintf(cli->name, sizeof(cli->name), "%s", sname);
	cli->drm = drm;
	mutex_init(&cli->mutex);

	INIT_WORK(&cli->work, nouveau_cli_work);
	INIT_LIST_HEAD(&cli->worker);
	mutex_init(&cli->lock);

	ret = nova_core_alloc_gsp_client(drm->auxdev,
					 &cli->gsp);
	if (ret)
		return -EINVAL;

	ret = nova_core_alloc_mmu(drm->auxdev,
				  &cli->mmu);
	if (ret) {
		NV_PRINTK(err, cli, "MMU allocation failed: %d\n", ret);
		goto done;
	}

	ret = nouveau_vmm_init(cli, &cli->vmm);
	if (ret) {
		NV_PRINTK(err, cli, "VMM allocation failed: %d\n", ret);
		goto done;
	}

	/* Don't pass in the (shared) sched_wq in order to let
	 * nouveau_sched_create() create a dedicated one for VM_BIND jobs.
	 *
	 * This is required to ensure that for VM_BIND jobs free_job() work and
	 * run_job() work can always run concurrently and hence, free_job() work
	 * can never stall run_job() work. For EXEC jobs we don't have this
	 * requirement, since EXEC job's free_job() does not require to take any
	 * locks which indirectly or directly are held for allocations
	 * elsewhere.
	 */
	ret = nouveau_sched_create(&cli->sched, drm, NULL, 1);
	if (ret)
		goto done;

	return 0;
done:
	if (ret)
		nouveau_cli_fini(cli);
	return ret;
}

static void
nouveau_accel_ce_fini(struct nouveau_drm *drm)
{
	nouveau_channel_idle(drm->cechan);
	nova_core_chan_free_object(&drm->ttm.copy);
	nouveau_channel_del(&drm->cechan);
}

static void
nouveau_accel_ce_init(struct nouveau_drm *drm)
{
	u64 runm;
	int ret;
	/* Allocate channel that has access to a (preferably async) copy
	 * engine, to use for TTM buffer moves.
	 */
	runm = novac_fifo_runlist_ce(&drm->info);
	if (!runm) {
		NV_ERROR(drm, "no ce runlist\n");
		return;
	}

	printk(KERN_ERR "Found ce %016llx\n", runm);

	ret = nouveau_channel_new(&drm->cli, false, runm, &drm->cechan);
	if (ret)
		NV_ERROR(drm, "failed to create ce channel, %d\n", ret);
}

static void
nouveau_accel_gr_fini(struct nouveau_drm *drm)
{
  	nouveau_channel_idle(drm->channel);
	nouveau_channel_del(&drm->channel);
}

static void
nouveau_accel_gr_init(struct nouveau_drm *drm)
{
	u64 runm;
	int ret;
	/* Allocate channel that has access to the graphics engine. */
	runm = novac_fifo_runlist(&drm->info, NOVA_CORE_ENGINE_GR);
	if (!runm) {
		NV_ERROR(drm, "no gr runlist\n");
		return;
	}

	printk(KERN_ERR "Found gr %016llx\n", runm);

	ret = nouveau_channel_new(&drm->cli, false, runm, &drm->channel);
	if (ret) {
		NV_ERROR(drm, "failed to create kernel channel, %d\n", ret);
		nouveau_accel_gr_fini(drm);
		return;
	}
}

static void
nouveau_accel_fini(struct nouveau_drm *drm)
{
	nouveau_accel_ce_fini(drm);
	nouveau_accel_gr_fini(drm);
	if (drm->fence)
		nouveau_fence(drm)->dtor(drm);
	nouveau_channels_fini(drm);
	nova_core_unmap_user(drm->auxdev, &drm->user);
}

static void
nouveau_accel_init(struct nouveau_drm *drm)
{
	int ret;
	/* Initialise global support for channels, and synchronisation. */
	ret = nouveau_channels_init(drm);
	if (ret)
		return;

	switch (drm->info.fifo_class) {
	case TURING_CHANNEL_GPFIFO_A:
	case AMPERE_CHANNEL_GPFIFO_A:
	case AMPERE_CHANNEL_GPFIFO_B:
		ret = nvc0_fence_create(drm);
		break;
	default:
		ret = -ENODEV;
		break;
	}

	if (ret) {
		NV_ERROR(drm, "failed to initialise sync subsystem, %d\n", ret);
		nouveau_accel_fini(drm);
		return;
	}

	/* Volta requires access to a doorbell register for kickoff. */
	ret = nova_core_map_user(drm->auxdev, &drm->user);
	if (ret)
		return;

	/* Allocate channels we need to support various functions. */
	nouveau_accel_gr_init(drm);
	nouveau_accel_ce_init(drm);

	/* Initialise accelerated TTM buffer moves. */
	nouveau_bo_move_init(drm);
}

static void
nouveau_drm_device_fini(struct nouveau_drm *drm)
{
	struct nouveau_cli *cli, *temp_cli;

	nouveau_debugfs_fini(drm);

	nouveau_accel_fini(drm);
	nouveau_ttm_fini(drm);

	/*
	 * There may be existing clients from as-yet unclosed files. For now,
	 * clean them up here rather than deferring until the file is closed,
	 * but this likely not correct if we want to support hot-unplugging
	 * properly.
	 */
	mutex_lock(&drm->clients_lock);
	list_for_each_entry_safe(cli, temp_cli, &drm->clients, head) {
		list_del(&cli->head);
		mutex_lock(&cli->mutex);
		if (cli->abi16)
			nouveau_abi16_fini(cli->abi16);
		mutex_unlock(&cli->mutex);
		nouveau_cli_fini(cli);
		kfree(cli);
	}
	mutex_unlock(&drm->clients_lock);

	nouveau_cli_fini(&drm->cli);
	destroy_workqueue(drm->sched_wq);
	mutex_destroy(&drm->clients_lock);
}

static int
nouveau_drm_device_init(struct nouveau_drm *drm)
{
	int ret;

	drm->sched_wq = alloc_workqueue("nouveau_sched_wq_shared", 0,
					WQ_MAX_ACTIVE);
	if (!drm->sched_wq)
		return -ENOMEM;

	ret = nouveau_cli_init(drm, "DRM", &drm->cli);
	if (ret)
		goto fail_wq;

	INIT_LIST_HEAD(&drm->clients);
	mutex_init(&drm->clients_lock);

	ret = nouveau_ttm_init(drm);
	if (ret)
		goto fail_ttm;

	nouveau_accel_init(drm);

	nouveau_debugfs_init(drm);

	ret = drm_dev_register(drm->dev, 0);
	if (ret) {
		nouveau_drm_device_fini(drm);
		return ret;
	}
	return 0;
fail_ttm:
	nouveau_cli_fini(&drm->cli);
fail_wq:
	destroy_workqueue(drm->sched_wq);
	return ret;
}

static void
nouveau_drm_device_del(struct nouveau_drm *drm)
{
        if (drm->dev)
                drm_dev_put(drm->dev);

        kfree(drm);
}

static struct nouveau_drm *
nouveau_drm_device_new(const struct drm_driver *drm_driver,
		       struct auxiliary_device *parent)
{
	struct nouveau_drm *drm;
	int ret = 0;

	drm = kzalloc(sizeof(*drm), GFP_KERNEL);
	if (!drm)
		return ERR_PTR(-ENOMEM);


	drm->auxdev = parent;

	drm->dev = drm_dev_alloc(drm_driver, parent->dev.parent);
	if (IS_ERR(drm->dev)) {
		ret = PTR_ERR(drm->dev);
		goto done;
	}

	drm->dev->dev_private = drm;
	auxiliary_set_drvdata(parent, drm);

	nova_core_fill_info(parent, &drm->info);
	printk(KERN_ERR "boot0 is %016llx\n", drm->info.boot0);

done:
	if (ret) {
		nouveau_drm_device_del(drm);
		drm = NULL;
	}
	return ret ? ERR_PTR(ret) : drm;
}


static struct drm_driver driver_stub;

static int
nouveau_drm_probe(struct auxiliary_device *auxdev, const struct auxiliary_device_id *id)
{
	struct nouveau_drm *drm;
	int ret;

	drm = nouveau_drm_device_new(&driver_stub, auxdev);
	if (IS_ERR(drm)) {
		ret = PTR_ERR(drm);
		goto fail_nvkm;
	}

	ret = nouveau_drm_device_init(drm);
	if (ret)
		return ret;

	return 0;
fail_nvkm:
	nouveau_drm_device_del(drm);
	return ret;
}
static void
nouveau_drm_remove(struct auxiliary_device *auxdev)
{
	struct nouveau_drm *drm = auxiliary_get_drvdata(auxdev);

	drm_dev_unplug(drm->dev);

	nouveau_drm_device_fini(drm);
	nouveau_drm_device_del(drm);

}
static int
nouveau_drm_open(struct drm_device *dev, struct drm_file *fpriv)
{
	struct nouveau_drm *drm = nouveau_drm(dev);
	struct nouveau_cli *cli;
	char name[32], tmpname[TASK_COMM_LEN];
	int ret = 0;

	get_task_comm(tmpname, current);
	rcu_read_lock();
	snprintf(name, sizeof(name), "%s[%d]",
		 tmpname, pid_nr(rcu_dereference(fpriv->pid)));
	rcu_read_unlock();

	if (!(cli = kzalloc(sizeof(*cli), GFP_KERNEL))) {
		ret = -ENOMEM;
		goto done;
	}

	ret = nouveau_cli_init(drm, name, cli);
	if (ret)
		goto done;

	fpriv->driver_priv = cli;

	mutex_lock(&drm->clients_lock);
	list_add(&cli->head, &drm->clients);
	mutex_unlock(&drm->clients_lock);
done:
	if (ret && cli) {
		nouveau_cli_fini(cli);
		kfree(cli);
	}
	return ret;
}

static void
nouveau_drm_postclose(struct drm_device *dev, struct drm_file *fpriv)
{
	struct nouveau_cli *cli = nouveau_cli(fpriv);
	struct nouveau_drm *drm = nouveau_drm(dev);
	int dev_index;

	/*
	 * The device is gone, and as it currently stands all clients are
	 * cleaned up in the removal codepath. In the future this may change
	 * so that we can support hot-unplugging, but for now we immediately
	 * return to avoid a double-free situation.
	 */
	if (!drm_dev_enter(dev, &dev_index))
		return;

	mutex_lock(&cli->mutex);
	if (cli->abi16)
		nouveau_abi16_fini(cli->abi16);
	mutex_unlock(&cli->mutex);

	mutex_lock(&drm->clients_lock);
	list_del(&cli->head);
	mutex_unlock(&drm->clients_lock);

	nouveau_cli_fini(cli);
	kfree(cli);
	drm_dev_exit(dev_index);
}


static const struct drm_ioctl_desc
nouveau_ioctls[] = {
	DRM_IOCTL_DEF_DRV(NOUVEAU_SETPARAM, drm_invalid_op, DRM_AUTH|DRM_MASTER|DRM_ROOT_ONLY),
	DRM_IOCTL_DEF_DRV(NOUVEAU_GETPARAM, nouveau_abi16_ioctl_getparam, DRM_RENDER_ALLOW),

	DRM_IOCTL_DEF_DRV(NOUVEAU_CHANNEL_ALLOC, nouveau_abi16_ioctl_channel_alloc, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(NOUVEAU_CHANNEL_FREE, nouveau_abi16_ioctl_channel_free, DRM_RENDER_ALLOW),
	//	DRM_IOCTL_DEF_DRV(NOUVEAU_SVM_INIT, nouveau_svmm_init, DRM_RENDER_ALLOW),
	//	DRM_IOCTL_DEF_DRV(NOUVEAU_SVM_BIND, nouveau_svmm_bind, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(NOUVEAU_GEM_NEW, nouveau_gem_ioctl_new, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(NOUVEAU_GEM_PUSHBUF, nouveau_gem_ioctl_pushbuf, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(NOUVEAU_GEM_CPU_PREP, nouveau_gem_ioctl_cpu_prep, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(NOUVEAU_GEM_CPU_FINI, nouveau_gem_ioctl_cpu_fini, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(NOUVEAU_GEM_INFO, nouveau_gem_ioctl_info, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(NOUVEAU_VM_INIT, nouveau_uvmm_ioctl_vm_init, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(NOUVEAU_VM_BIND, nouveau_uvmm_ioctl_vm_bind, DRM_RENDER_ALLOW),
	DRM_IOCTL_DEF_DRV(NOUVEAU_EXEC, nouveau_exec_ioctl_exec, DRM_RENDER_ALLOW),
};

long
nouveau_drm_ioctl(struct file *file, unsigned int cmd, unsigned long arg)
{
	struct drm_file *filp = file->private_data;
	long ret;

	switch (_IOC_NR(cmd) - DRM_COMMAND_BASE) {
	case DRM_NOUVEAU_NVIF:
		ret = nouveau_abi16_ioctl(filp, (void __user *)arg, _IOC_SIZE(cmd));
		break;
	default:
		ret = drm_ioctl(file, cmd, arg);
		break;
	}

	return ret;
}

static const struct file_operations
nouveau_driver_fops = {
	.owner = THIS_MODULE,
	.open = drm_open,
	.release = drm_release,
	.unlocked_ioctl = nouveau_drm_ioctl,
	.mmap = drm_gem_mmap,
	.poll = drm_poll,
	.read = drm_read,
#if defined(CONFIG_COMPAT)
	.compat_ioctl = nouveau_compat_ioctl,
#endif
	.llseek = noop_llseek,
	.fop_flags = FOP_UNSIGNED_OFFSET,
};

static struct drm_driver
driver_stub = {
  	.driver_features = DRIVER_GEM |
			   DRIVER_SYNCOBJ | DRIVER_SYNCOBJ_TIMELINE |
			   DRIVER_GEM_GPUVA |
			   DRIVER_RENDER,
#if defined(CONFIG_DEBUG_FS)
	.debugfs_init = nouveau_drm_debugfs_init,
#endif
	.open = nouveau_drm_open,
	.postclose = nouveau_drm_postclose,

	.ioctls = nouveau_ioctls,
	.num_ioctls = ARRAY_SIZE(nouveau_ioctls),
	.fops = &nouveau_driver_fops,

	.gem_prime_import_sg_table = nouveau_gem_prime_import_sg_table,
	.name = DRIVER_NAME,
	.desc = DRIVER_DESC,
#ifdef GIT_REVISION
	.date = GIT_REVISION,
#else
	.date = DRIVER_DATE,
#endif
	.major = DRIVER_MAJOR,
	.minor = DRIVER_MINOR,
	.patchlevel = DRIVER_PATCHLEVEL,
};

static const struct auxiliary_device_id
novac_drm_id_table[] = {
	{ .name = "NovaCore.device" },
	{}
};

static struct auxiliary_driver
novac_auxdrv = {
	.name = "nouveau",
	.id_table = novac_drm_id_table,
	.probe = nouveau_drm_probe,
	.remove = nouveau_drm_remove,
	//	.driver.pm = &nouveau_pm_ops,
};

static int __init
novac_drm_init(void)
{
	return auxiliary_driver_register(&novac_auxdrv);
}

static void __exit
novac_drm_exit(void)
{
	auxiliary_driver_unregister(&novac_auxdrv);
}

module_init(novac_drm_init);
module_exit(novac_drm_exit);

MODULE_AUTHOR(DRIVER_AUTHOR);
MODULE_DESCRIPTION(DRIVER_DESC);
MODULE_LICENSE("GPL and additional rights");
