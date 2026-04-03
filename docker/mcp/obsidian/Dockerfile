FROM node:22-slim
WORKDIR /app
COPY package.json .
RUN npm install --production
COPY app.js .
COPY MCP.md .
EXPOSE 8000
CMD ["node", "app.js"]
