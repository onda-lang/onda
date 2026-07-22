# Third-party notices

The packaged compiler includes the following third-party components:

- Binaryen.js 130.0.0, licensed under Apache-2.0. The complete license is shipped as
  `dist/licenses/BINARYEN-LICENSE`.
- The Onda WebAssembly math kernel uses `libm` 0.2.16, licensed under MIT. The complete license is
  shipped as `dist/licenses/LIBM-LICENSE`.

The generated processor artifacts contain only the required closure of the math kernel. They do
not contain Binaryen itself.
