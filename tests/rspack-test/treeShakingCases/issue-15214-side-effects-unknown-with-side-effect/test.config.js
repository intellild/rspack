module.exports = {
	snapshotContent(content) {
		return `server-only-package included: ${content.includes(
			"IMPURE_SERVER_ONLY_PACKAGE"
		)}`;
	}
};
