FROM python:3.12-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends nodejs && \
    rm -rf /var/lib/apt/lists/* && \
    pip install --no-cache-dir fastapi uvicorn

COPY app.py /bridge/app.py
WORKDIR /bridge
EXPOSE 8000
CMD ["uvicorn", "app:app", "--host", "0.0.0.0", "--port", "8000"]
