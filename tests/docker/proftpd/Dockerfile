FROM alpine:3.21
RUN apk add --no-cache proftpd proftpd-mod_tls netcat-openbsd \
 && adduser -D -h /home/ftpuser ftpuser && echo 'ftpuser:ftppass' | chpasswd
RUN mkdir -p /run/proftpd /var/run/proftpd
COPY *.conf /etc/proftpd/
ENV CONF=explicit.conf
CMD ["sh", "-c", "exec proftpd --nodaemon --config /etc/proftpd/$CONF"]
