/** @type {import("@rspack/core").Configuration} */
module.exports = {
  target: 'node14',
  experiments: {
    outputModule: true,
  },
  output: {
    filename: 'main.mjs',
    cssFilename: 'style.css',
    module: true,
  },
  module: {
    parser: {
      javascript: {
        importMeta: false,
      },
    },
    rules: [
      {
        test: /\.scss$/,
        use: [{ loader: 'sass-loader' }],
        type: 'css',
        generator: {
          exportsOnly: false,
        },
      },
    ],
  },
};
