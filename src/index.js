const details = document.getElementById("details");
const state = document.getElementById("state");

document.getElementById("initPushBtn").addEventListener("click", main);
document.getElementById("initSseBtn").addEventListener("click", serverSentEvent);

function urlBase64ToUint8Array(base64String) {
    var padding = "=".repeat((4 - (base64String.length % 4)) % 4);
    var base64 = (base64String + padding).replace(/\-/g, "+").replace(/_/g, "/");

    var rawData = window.atob(base64);
    var outputArray = new Uint8Array(rawData.length);

    for (var i = 0; i < rawData.length; ++i) {
        outputArray[i] = rawData.charCodeAt(i);
    }
    return outputArray;
}

async function fetchVapidKeys() {
    return fetch("/vapid.json").then((resp) => resp.json());
}

async function subscribeUserToPush(vapidKeys) {
    const registration = await navigator.serviceWorker.register("service_worker.js");
    registration.update();
    const pushSubscription = await registration.pushManager.subscribe({
        userVisibleOnly: true,
        applicationServerKey: urlBase64ToUint8Array(vapidKeys.publicKey)
    });
    return pushSubscription;
}

async function main() {
    try {
        let keys = await fetchVapidKeys();
        await Notification.requestPermission();
        state.subscription = await subscribeUserToPush(keys);
        details.textContent = JSON.stringify(state.subscription, null, 4);
    } catch (error) {
        if (error instanceof Error) {
            state.innerText = error.message;
        }
    }

    try {
        await fetch("/register", {
            method: "POST",
            headers: {
                "Content-Type": "application/json"
            },
            body: JSON.stringify({
                user_id: document.getElementById("userId").value,
                ...JSON.parse(JSON.stringify(state.subscription))
            })
        });
    } catch (error) {
        if (error instanceof Error) {
            state.innerText = error.message;
        }
    }
}

function serverSentEvent() {
    const eventSource = new EventSource(`/sse?user_id=${document.getElementById("userId").value}`);
    eventSource.onmessage = (event) => {
        state.textContent = (state.textContent ?? "") + "\n" + event.data;
    };
}
