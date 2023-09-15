self.addEventListener("activate", () => {
    clients.claim();
});

self.addEventListener("install", () => {
    self.skipWaiting();
});

self.addEventListener("push", async (event) => {
    try {
        const data = event.data.json();
        const options = {
            body: data.body
        };
        await self.registration.showNotification(data.title, options);
    } catch (error) {
        console.log(error);
    }
});
