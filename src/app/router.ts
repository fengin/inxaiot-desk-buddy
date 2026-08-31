import { createRouter, createWebHashHistory } from "vue-router";

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", redirect: "/aio/nodes" },
    {
      path: "/aio/nodes",
      name: "aio-nodes",
      component: () => import("@/features/aio/nodes/AioNodeListView.vue")
    },
    {
      path: "/aio/release",
      name: "aio-release",
      component: () => import("@/features/aio/release-profile/AioReleaseProfileView.vue")
    },
    {
      path: "/aio/operations",
      name: "aio-operations",
      component: () => import("@/features/aio/operations/AioOperationsView.vue")
    }
  ]
});

