import { rspack } from '@rspack/core';

export default {
  module: {
    rules: [
      {
        test: /value\.js$/,
        use: [
          { loader: './loader.js', parallel: { maxWorkers: 1 }, options: {} },
        ],
      },
    ],
  },
  plugins: [
    {
      apply(compiler) {
        compiler.hooks.compilation.tap('NativeWorkerModule', (compilation) => {
          compilation.hooks.processAssets.tap('NativeWorkerModule', () => {
            const asset = compilation.getAsset('worker-module.txt');
            expect(asset.source.source()).toBe('from native worker');
            expect(asset.info.immutable).toBe(true);
            expect(asset.info.sourceFilename).toBe('value.js');
          });
          rspack.NormalModule.getCompilationHooks(compilation).loader.tap(
            'NativeWorkerModule',
            (context, module) => {
              if (!module.resource?.endsWith('value.js')) return;
              module.buildInfo.fromHook = () => 'main hook';
              // Extensions cannot replace the worker's native-backed facade.
              context._module = { replacedByMainHook: true };
            },
          );
        });
        compiler.hooks.finishMake.tap('NativeWorkerModule', (compilation) => {
          const module = [...compilation.modules].find((m) =>
            m.resource?.endsWith('value.js'),
          );
          expect(module.factoryMeta.sideEffectFree).toBe(false);
          expect(module.matchResource).toBe('worker-resource.js');
          expect(module.buildInfo.nativeWorker).toBe(true);
        });
      },
    },
  ],
};
