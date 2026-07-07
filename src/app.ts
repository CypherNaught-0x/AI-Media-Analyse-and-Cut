import { createApp } from "vue";
import "./style.css";
import "flag-icons/css/flag-icons.min.css";
import App from "./App.vue";
import router from "./router";

export function mountApp() {
  createApp(App).use(router).mount("#app");
}
