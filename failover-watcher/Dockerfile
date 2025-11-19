FROM node:20-alpine

WORKDIR /app

COPY package*.json ./
RUN npm ci --production

COPY src ./src

USER node

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \
  CMD node -e "process.exit(0)" || exit 1

CMD ["node", "src/index.js"]
