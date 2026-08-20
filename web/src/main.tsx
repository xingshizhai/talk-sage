import ReactDOM from "react-dom/client";
import App from "./App";

// 注意：不用 <React.StrictMode>——其开发模式 double-invoke effects 会让
// 领域事件监听器注册两次，导致每句话在转写界面显示两遍。
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<App />);
