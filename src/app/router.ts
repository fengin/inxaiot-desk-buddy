import { createRouter, createWebHashHistory } from "vue-router";

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", redirect: import.meta.env.DEV && import.meta.env.VITE_SCREEN_PROTOTYPE === "1" ? "/screen/nodes" : "/aio/nodes" },
    {
      path: "/aio/nodes",
      name: "aio-nodes",
      meta: { projectAccess: "shared" },
      component: () => import("@/features/aio/nodes/AioNodeListView.vue")
    },
    {
      path: "/aio/release",
      name: "aio-release",
      meta: { projectAccess: "shared" },
      component: () => import("@/features/aio/release-profile/AioReleaseProfileView.vue")
    },
    {
      path: "/aio/operations",
      name: "aio-operations",
      meta: { projectAccess: "shared" },
      component: () => import("@/features/aio/operations/AioOperationsView.vue")
    },
    { path: "/screen/nodes", name: "screen-nodes", meta: { projectAccess: "local" }, component: () => import("@/features/smart-screen/ScreenWorkspace.vue") },
    { path: "/screen/operations", name: "screen-operations", meta: { projectAccess: "local" }, component: () => import("@/features/smart-screen/ScreenWorkspace.vue") }
  ]
});
