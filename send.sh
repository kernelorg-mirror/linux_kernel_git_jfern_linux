#!/bin/bash
git send-email							\
    --to-cmd='/tmp/to.sh'					\
    --cc-cmd='/tmp/cc.sh'					\
	$(cat /tmp/gitsend.extra.args) /tmp/p
