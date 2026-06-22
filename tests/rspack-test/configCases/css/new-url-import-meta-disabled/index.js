const url = new URL("./style.scss", import.meta.url);

it("should preserve new URL for css handled by module rules", () => {
	expect(url.pathname.endsWith("/style.css")).toBe(true);
});
