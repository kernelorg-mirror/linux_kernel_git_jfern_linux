#include <linux/auxiliary_bus.h>

void rust_helper_auxiliary_set_drvdata(struct auxiliary_device *auxdev, void *data)
{
	auxiliary_set_drvdata(auxdev, data);
}

void *rust_helper_auxiliary_get_drvdata(struct auxiliary_device *auxdev)
{
	return auxiliary_get_drvdata(auxdev);
}
