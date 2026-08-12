import binding from '@rspack/binding';
import * as liteTapable from '@rspack/lite-tapable';

import { type Compilation, checkCompilation } from '../Compilation';
import type { CreatePartialRegisters } from '../taps/types';
import { create } from './base';

const TemporaryBuiltinPluginImpl = create(
  binding.BuiltinPluginName.TemporaryBuiltinPlugin,
  () => {},
  'compilation',
);

export type TemporaryBuiltinPluginHooks = {
  temporary: liteTapable.SyncHook<[Compilation]>;
};

const TemporaryBuiltinPlugin =
  TemporaryBuiltinPluginImpl as typeof TemporaryBuiltinPluginImpl & {
    getCompilationHooks(compilation: Compilation): TemporaryBuiltinPluginHooks;
  };

const temporaryBuiltinHooksOwner =
  binding.ExternalModule as typeof binding.ExternalModule & {
    getTemporaryBuiltinCompilationHooks(
      compilation: Compilation,
    ): TemporaryBuiltinPluginHooks;
  };

const compilationHooksMap = new WeakMap<
  Compilation,
  TemporaryBuiltinPluginHooks
>();

Object.defineProperty(
  temporaryBuiltinHooksOwner,
  'getTemporaryBuiltinCompilationHooks',
  {
    configurable: true,
    value(compilation: Compilation) {
      checkCompilation(compilation);

      let hooks = compilationHooksMap.get(compilation);
      if (hooks === undefined) {
        hooks = {
          temporary: new liteTapable.SyncHook(['compilation']),
        };
        compilationHooksMap.set(compilation, hooks);
      }
      return hooks;
    },
  },
);

TemporaryBuiltinPlugin.getCompilationHooks = (compilation: Compilation) =>
  temporaryBuiltinHooksOwner.getTemporaryBuiltinCompilationHooks(compilation);

export { TemporaryBuiltinPlugin };

export const createTemporaryBuiltinPluginHooksRegisters: CreatePartialRegisters<
  'TemporaryBuiltin'
> = (getCompiler, createTap) => ({
  registerTemporaryBuiltinTaps: createTap(
    binding.RegisterJsTapKind.TemporaryBuiltin,
    () =>
      temporaryBuiltinHooksOwner.getTemporaryBuiltinCompilationHooks(
        getCompiler().__internal__get_compilation()!,
      ).temporary,
    (queried) => (compilation: Compilation) => queried.call(compilation),
  ),
});
