const fs = require("fs");
const path = require("path");

/** @type {import("../../../..").TConfigCaseConfig} */
module.exports = {
	findBundle(i, options) {
		const source = fs.readFileSync(
			path.resolve(options.output.path, "main.mjs"),
			"utf-8"
		);

		expect(source).toContain('new URL("./style.css", import.meta.url)');
		expect(source).not.toContain("asset import");
		expect(fs.existsSync(path.resolve(options.output.path, "style.css"))).toBe(
			true
		);

		return "main.mjs";
	}
};
