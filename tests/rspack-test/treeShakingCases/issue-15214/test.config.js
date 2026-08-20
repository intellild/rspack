module.exports = {
	snapshotContent(content) {
		return `server-only-package included: ${content.includes(
			"SERVER_ONLY_PACKAGE"
		)}`;
	}
};
