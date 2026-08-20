module.exports = {
	snapshotContent(content) {
		return `server-only-package included: ${content.includes(
			"PURE_SERVER_ONLY_PACKAGE"
		)}`;
	}
};
