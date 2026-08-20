const { doSomething } = require("server-only-package");

if (import.meta.env.SSR) {
	doSomething();
}
