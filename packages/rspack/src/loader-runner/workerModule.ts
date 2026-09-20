import type { BuildInfo, Module, NormalModule } from '@rspack/binding';

/** The worker API deliberately excludes compilation graph access. */
export function createWorkerModule(native: Module, buildInfo: BuildInfo) {
  let active = true;
  const getNative = () => {
    if (!active)
      throw new Error(
        'Worker Module is unavailable after its loader task has completed',
      );
    return native as NormalModule;
  };
  const module = {
    get type() {
      return getNative().type;
    },
    get context() {
      return getNative().context;
    },
    get resource() {
      return getNative().resource;
    },
    get request() {
      return getNative().request;
    },
    get userRequest() {
      return getNative().userRequest;
    },
    get rawRequest() {
      return getNative().rawRequest;
    },
    get matchResource() {
      return getNative().matchResource;
    },
    set matchResource(value) {
      getNative().matchResource = value;
    },
    get layer() {
      return getNative().layer;
    },
    get useSourceMap() {
      return getNative().useSourceMap;
    },
    get useSimpleSourceMap() {
      return getNative().useSimpleSourceMap;
    },
    get factoryMeta() {
      return getNative().factoryMeta;
    },
    set factoryMeta(value) {
      getNative().factoryMeta = value;
    },
    get buildInfo() {
      getNative();
      // Custom values may include functions. Keep the existing object bridge,
      // including assets, whose reflector belongs to the main JS env.
      return buildInfo;
    },
    identifier() {
      return getNative().identifier();
    },
    readableIdentifier() {
      return getNative().readableIdentifier();
    },
    nameForCondition() {
      return getNative().nameForCondition();
    },
    originalSource() {
      return getNative().originalSource();
    },
    size(...args: Parameters<Module['size']>) {
      return getNative().size(...args);
    },
    libIdent(...args: Parameters<Module['libIdent']>) {
      return getNative().libIdent(...args);
    },
    emitFile(...args: Parameters<Module['emitFile']>) {
      return getNative().emitFile(...args);
    },
  };
  return {
    module: module as NormalModule,
    dispose() {
      active = false;
    },
  };
}
