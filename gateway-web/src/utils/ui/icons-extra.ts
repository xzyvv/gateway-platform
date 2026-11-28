/**
 * 非首屏图标注册（懒加载）。
 *
 * 登录页只需 Cellphone/Search/User/Lock/InfoFilled 5 个图标，其余 78 个在
 * 用户登录后才用到。把这 78 个从 main.ts 拆出，挂载后再动态导入注册，
 * 首屏 el-icons 体积从 83 个(120KB) 降到 5 个(~7KB)，登录页白屏明显缩短。
 *
 * 注册时机：bootstrap() 中 app.mount() 之后、用户有交互之前完成。
 * 登录流程本身要走网络请求（validate），足够覆盖动态导入耗时。
 */
import type { App } from 'vue'
import {
  ArrowDown, ArrowRight, Back, Bottom, Box, ChatDotRound, Check, CircleCheck,
  CircleCheckFilled, CircleClose, CircleCloseFilled, Clock, Close, Coin, Collection,
  CollectionTag, Compass, Connection, CopyDocument, Cpu, DArrowRight, DataAnalysis,
  DataLine, Delete, Document, DocumentCopy, Download, Edit, EditPen, Expand, Files,
  FirstAidKit, Fold, Folder, FolderAdd, FolderOpened, Grid, Histogram, Key, Link, Loading,
  Location, MagicStick, MessageBox, Minus, Monitor, Moon, Odometer, Operation, Plus,
  PriceTag, Promotion, QuestionFilled, Reading, Refresh, RefreshLeft, RefreshRight,
  Right, Setting, Share, Sort, Star, StarFilled, Sunny, SwitchButton, Timer, Top,
  TrendCharts, Trophy, Upload, UploadFilled, UserFilled, VideoCamera, VideoPause,
  VideoPlay, View, Warning, WarningFilled,
} from '@element-plus/icons-vue'

const extraIcons = {
  ArrowDown, ArrowRight, Back, Bottom, Box, ChatDotRound, Check, CircleCheck,
  CircleCheckFilled, CircleClose, CircleCloseFilled, Clock, Close, Coin, Collection,
  CollectionTag, Compass, Connection, CopyDocument, Cpu, DArrowRight, DataAnalysis,
  DataLine, Delete, Document, DocumentCopy, Download, Edit, EditPen, Expand, Files,
  FirstAidKit, Fold, Folder, FolderAdd, FolderOpened, Grid, Histogram, Key, Link, Loading,
  Location, MagicStick, MessageBox, Minus, Monitor, Moon, Odometer, Operation, Plus,
  PriceTag, Promotion, QuestionFilled, Reading, Refresh, RefreshLeft, RefreshRight,
  Right, Setting, Share, Sort, Star, StarFilled, Sunny, SwitchButton, Timer, Top,
  TrendCharts, Trophy, Upload, UploadFilled, UserFilled, VideoCamera, VideoPause,
  VideoPlay, View, Warning, WarningFilled,
}

export function registerExtraIcons(app: App): void {
  for (const [key, component] of Object.entries(extraIcons)) {
    app.component(key, component)
  }
}
