module.exports = function () {
  const callback = this.async();
  // Overlap callbacks from different compilers to exercise compiler routing.
  setTimeout(() => callback(null, `module.exports = ${JSON.stringify(`subscription:${this._compiler.options.name}`)};`), 5);
};
