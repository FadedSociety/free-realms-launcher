# Free Realms Expanded realm directory: one small process, list kept in memory (see directory.py).
FROM python:3.12-slim
RUN pip install --no-cache-dir -r /dev/stdin <<'REQ'
cryptography==44.0.0
REQ
COPY directory.py /app/directory.py
ENV PYTHONUNBUFFERED=1
USER nobody
CMD ["python", "/app/directory.py"]
