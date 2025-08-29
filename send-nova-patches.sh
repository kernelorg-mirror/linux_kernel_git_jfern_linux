#!/bin/bash

git send-email \
  --to=linux-kernel@vger.kernel.org \
  --to=rust-for-linux@vger.kernel.org \
  --to=dri-devel@lists.freedesktop.org \
  --to=dakr@kernel.org \
  --to=acourbot@nvidia.com \
  --cc="Alistair Popple <apopple@nvidia.com>" \
  --cc="Miguel Ojeda <ojeda@kernel.org>" \
  --cc="Alex Gaynor <alex.gaynor@gmail.com>" \
  --cc="Boqun Feng <boqun.feng@gmail.com>" \
  --cc="Gary Guo <gary@garyguo.net>" \
  --cc="bjorn3_gh@protonmail.com" \
  --cc="Benno Lossin <lossin@kernel.org>" \
  --cc="Andreas Hindborg <a.hindborg@kernel.org>" \
  --cc="Alice Ryhl <aliceryhl@google.com>" \
  --cc="Trevor Gross <tmgross@umich.edu>" \
  --cc="David Airlie <airlied@gmail.com>" \
  --cc="Simona Vetter <simona@ffwll.ch>" \
  --cc="Maarten Lankhorst <maarten.lankhorst@linux.intel.com>" \
  --cc="Maxime Ripard <mripard@kernel.org>" \
  --cc="Thomas Zimmermann <tzimmermann@suse.de>" \
  --cc="John Hubbard <jhubbard@nvidia.com>" \
  --cc="Joel Fernandes <joelagnelf@nvidia.com>" \
  --cc="Timur Tabi <ttabi@nvidia.com>" \
  --cc="joel@joelfernandes.org" \
  --cc="Elle Rhumsaa <elle@weathered-steel.dev>" \
  --cc="Yury Norov <yury.norov@gmail.com>" \
  --cc="Daniel Almeida <daniel.almeida@collabora.com>" \
  --cc="Andrea Righi <arighi@nvidia.com>" \
  --cc=nouveau@lists.freedesktop.org \
  *patch

