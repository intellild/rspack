import { doSomething } from "server-only-package";

if (import.meta.env.SSR) {
	doSomething();
}
